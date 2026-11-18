use gtk::{
    gdk_pixbuf::{Colorspace, InterpType, Pixbuf},
    gio,
};
use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Current,
    Queue,
}

type Key = (PathBuf, Kind);

#[derive(Default)]
struct Cache {
    current: VecDeque<(PathBuf, Option<Rc<Artwork>>)>,
    queue: VecDeque<(PathBuf, Option<Rc<Artwork>>)>,
    pending: HashSet<Key>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

pub(crate) struct Artwork {
    pub hero: Option<Pixbuf>,
    pub mini: Option<Pixbuf>,
    pub thumb: Pixbuf,
    pub backdrop: Option<Pixbuf>,
    pub hue: Option<f64>,
}

struct Decoded {
    hero: Option<Pixels>,
    mini: Option<Pixels>,
    thumb: Pixels,
    backdrop: Option<Pixels>,
    hue: Option<f64>,
}

struct Pixels {
    bytes: glib::Bytes,
    size: i32,
    stride: i32,
}

impl Pixels {
    fn pixbuf(self) -> Pixbuf {
        Pixbuf::from_bytes(
            &self.bytes,
            Colorspace::Rgb,
            true,
            8,
            self.size,
            self.size,
            self.stride,
        )
    }
}

impl Cache {
    fn entries(&mut self, kind: Kind) -> &mut VecDeque<(PathBuf, Option<Rc<Artwork>>)> {
        match kind {
            Kind::Current => &mut self.current,
            Kind::Queue => &mut self.queue,
        }
    }
}

// Outer None means pending. Inner None means the path is absent or failed to decode.
pub(crate) fn get(path: Option<&Path>, current: bool) -> Option<Option<Rc<Artwork>>> {
    let Some(path) = path else { return Some(None) };
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(index) = cache.current.iter().position(|(key, _)| key == path) {
            let entry = cache.current.remove(index).unwrap();
            let result = entry.1.clone();
            cache.current.push_front(entry);
            return Some(result);
        }
        let kind = if current { Kind::Current } else { Kind::Queue };
        let entries = cache.entries(kind);
        if let Some(index) = entries.iter().position(|(key, _)| key == path) {
            let entry = entries.remove(index).unwrap();
            let result = entry.1.clone();
            entries.push_front(entry);
            return Some(result);
        }
        // Reserve capacity for the selected track when the queue is scrolling.
        let key = (path.to_owned(), kind);
        if cache.pending.len() >= if current { 6 } else { 4 }
            || (!current && cache.pending.contains(&(key.0.clone(), Kind::Current)))
            || !cache.pending.insert(key.clone())
        {
            return None;
        }
        let source = key.0.clone();
        let task = gio::spawn_blocking(move || decode(&source, kind));
        glib::MainContext::default().spawn_local(async move {
            let result = task.await.ok().flatten().map(|image| {
                Rc::new(Artwork {
                    hero: image.hero.map(Pixels::pixbuf),
                    mini: image.mini.map(Pixels::pixbuf),
                    thumb: image.thumb.pixbuf(),
                    backdrop: image.backdrop.map(Pixels::pixbuf),
                    hue: image.hue,
                })
            });
            CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                cache.pending.remove(&key);
                let entries = cache.entries(kind);
                entries.push_front((key.0, result));
                entries.truncate(if kind == Kind::Current { 12 } else { 128 });
            });
        });
        None
    })
}

fn decode(path: &Path, kind: Kind) -> Option<Decoded> {
    let limit = if kind == Kind::Current { 288 } else { 44 };
    let (width, height) = Pixbuf::file_info(path).map(|(_, width, height)| (width, height))?;
    let shortest = width.min(height);
    if shortest <= 0 {
        return None;
    }
    let longest = width.max(height);
    // Keep wide or tall artwork bounded even when its short edge is already small.
    let ratio = (f64::from(limit) / f64::from(shortest))
        .min(f64::from(limit * 4) / f64::from(longest))
        .min(1.0);
    let pixbuf = Pixbuf::from_file_at_scale(
        path,
        (f64::from(width) * ratio).ceil() as i32,
        (f64::from(height) * ratio).ceil() as i32,
        true,
    )
    .ok()?;
    let current = kind == Kind::Current;
    let hue = current
        .then(|| {
            let ratio = 48.0 / f64::from(pixbuf.width().max(pixbuf.height()));
            let small = pixbuf.scale_simple(
                (f64::from(pixbuf.width()) * ratio).round().max(1.0) as i32,
                (f64::from(pixbuf.height()) * ratio).round().max(1.0) as i32,
                InterpType::Bilinear,
            )?;
            super::theme::cover_hue(&small)
        })
        .flatten();
    Some(Decoded {
        hero: current.then(|| rounded(&pixbuf, 288, 24.0)).flatten(),
        mini: current.then(|| rounded(&pixbuf, 48, 8.0)).flatten(),
        thumb: rounded(&pixbuf, 44, 8.0)?,
        backdrop: current.then(|| rounded(&pixbuf, 32, 0.0)).flatten(),
        hue,
    })
}

fn rounded(pixbuf: &Pixbuf, size: i32, radius: f64) -> Option<Pixels> {
    let square = pixbuf.width().min(pixbuf.height());
    if square <= 0 {
        return None;
    }
    let x = (pixbuf.width() - square) / 2;
    let y = (pixbuf.height() - square) / 2;
    let cropped = pixbuf.new_subpixbuf(x, y, square, square);
    let scaled = cropped.scale_simple(size, size, InterpType::Bilinear)?;
    let scaled = if scaled.has_alpha() {
        scaled
    } else {
        scaled.add_alpha(false, 0, 0, 0).ok()?
    };
    let stride = scaled.rowstride() as usize;
    let bytes = scaled.read_pixel_bytes();
    if radius == 0.0 {
        return Some(Pixels {
            bytes,
            size,
            stride: stride as i32,
        });
    }
    let mut data = bytes.as_ref().to_vec();
    let edge = size as f64;
    let radius = radius.min(edge / 2.0);
    let corner = radius.ceil() as i32;
    for y in 0..corner {
        for x in 0..corner {
            let dx = (radius - f64::from(x) - 0.5).max(0.0);
            let dy = (radius - f64::from(y) - 0.5).max(0.0);
            let alpha = (radius - dx.hypot(dy) + 0.5).clamp(0.0, 1.0);
            if alpha < 1.0 {
                for py in [y, size - 1 - y] {
                    for px in [x, size - 1 - x] {
                        let offset = py as usize * stride + px as usize * 4 + 3;
                        data[offset] = (f64::from(data[offset]) * alpha) as u8;
                    }
                }
            }
        }
    }
    Some(Pixels {
        bytes: glib::Bytes::from_owned(data),
        size,
        stride: stride as i32,
    })
}
