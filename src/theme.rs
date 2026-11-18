use gtk::prelude::*;

const LIGHT: &str = "
@define-color md_primary #176573;
@define-color md_on_primary #ffffff;
@define-color md_primary_container #aaedf9;
@define-color md_on_primary_container #001f25;
@define-color md_secondary_container #cde7ed;
@define-color md_on_secondary_container #0b1f24;
@define-color md_tertiary_container #dce2ff;
@define-color md_on_tertiary_container #151b38;
@define-color md_surface #f5fafb;
@define-color md_surface_low #edf4f5;
@define-color md_stage #edf4f5;
@define-color md_surface_container #e7eef0;
@define-color md_surface_high #e0e8ea;
@define-color md_on_surface #161d1f;
@define-color md_on_surface_variant #3e484b;
@define-color md_outline #6d797c;
@define-color md_outline_variant #bdc9cc;
@define-color md_inverse_surface #2b3234;
@define-color md_inverse_on_surface #edf4f5;
@define-color md_error #ba1a1a;
";

const DARK: &str = "
@define-color md_primary #8BCAD8;
@define-color md_on_primary #00363f;
@define-color md_primary_container #004e5a;
@define-color md_on_primary_container #aaedf9;
@define-color md_secondary_container #334a50;
@define-color md_on_secondary_container #cde7ed;
@define-color md_tertiary_container #3d4565;
@define-color md_on_tertiary_container #dce2ff;
@define-color md_surface #151B1E;
@define-color md_surface_low #192125;
@define-color md_stage #192125;
@define-color md_surface_container #20292D;
@define-color md_surface_high #283338;
@define-color md_on_surface #E5E9EB;
@define-color md_on_surface_variant #9AAEB6;
@define-color md_outline #879497;
@define-color md_outline_variant #3e484b;
@define-color md_inverse_surface #E5E9EB;
@define-color md_inverse_on_surface #2b3234;
@define-color md_error #ffb4ab;
";

thread_local! {
    static THEME: std::cell::OnceCell<Theme> = const { std::cell::OnceCell::new() };
}

struct Theme {
    provider: gtk::CssProvider,
    settings: gtk::Settings,
    tint: gtk::CssProvider,
    hue: std::cell::Cell<Option<f64>>,
    dynamic: std::cell::Cell<bool>,
    motion: std::cell::Cell<bool>,
}

pub(crate) fn install(screen: &gtk::gdk::Screen) {
    THEME.with(|theme| {
        theme.get_or_init(|| {
            let provider = gtk::CssProvider::new();
            let settings = gtk::Settings::for_screen(screen).expect("GTK screen settings");
            gtk::StyleContext::add_provider_for_screen(
                screen,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            settings.connect_gtk_theme_name_notify(|_| refresh());
            settings.connect_notify_local(Some("gtk-enable-animations"), |_, _| refresh());
            settings.connect_gtk_application_prefer_dark_theme_notify(|_| refresh());
            let tint = gtk::CssProvider::new();
            gtk::StyleContext::add_provider_for_screen(
                screen,
                &tint,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            Theme {
                provider,
                tint,
                settings,
                hue: std::cell::Cell::new(None),
                dynamic: std::cell::Cell::new(true),
                motion: std::cell::Cell::new(true),
            }
        });
    });
    refresh();
}

pub(crate) fn set_hue(hue: Option<f64>) {
    THEME.with(|theme| {
        if let Some(theme) = theme.get() {
            if theme.hue.replace(hue) != hue && theme.dynamic.get() {
                refresh_tint(theme);
            }
        }
    });
}

pub(crate) fn set_dynamic(enabled: bool) {
    THEME.with(|theme| {
        if let Some(theme) = theme.get() {
            if theme.dynamic.replace(enabled) != enabled {
                refresh_tint(theme);
            }
        }
    });
}

pub(crate) fn set_motion(enabled: bool) {
    let changed = THEME.with(|theme| {
        theme
            .get()
            .is_some_and(|theme| theme.motion.replace(enabled) != enabled)
    });
    if changed {
        refresh();
    }
}

#[cfg(test)]
pub(crate) fn set_cover(path: Option<&std::path::Path>) {
    set_hue(
        path.and_then(|path| gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, 48, 48, true).ok())
            .as_ref()
            .and_then(cover_hue),
    );
}

fn dark_scheme(settings: &gtk::Settings) -> bool {
    match std::env::var("GTK_THEME")
        .unwrap_or_default()
        .rsplit_once(':')
    {
        Some((_, "dark")) => true,
        Some((_, "light")) => false,
        _ => {
            settings.is_gtk_application_prefer_dark_theme()
                || settings
                    .gtk_theme_name()
                    .is_some_and(|name| name.to_ascii_lowercase().contains("dark"))
        }
    }
}

fn refresh() {
    THEME.with(|theme| {
        let Some(theme) = theme.get() else { return };
        let baseline = if dark_scheme(&theme.settings) {
            DARK
        } else {
            LIGHT
        };
        let motion =
            if theme.motion.get() && theme.settings.property::<bool>("gtk-enable-animations") {
                ""
            } else {
                "* { transition: none; }"
            };
        let css = format!("{baseline}{}{motion}", include_str!("style.css"));
        if let Err(error) = theme.provider.load_from_data(css.as_bytes()) {
            eprintln!("Не удалось загрузить стиль: {error}");
        }
        refresh_tint(theme);
    });
}

fn refresh_tint(theme: &Theme) {
    let css = if let Some(hue) = theme.hue.get().filter(|_| theme.dynamic.get()) {
        let tone = if dark_scheme(&theme.settings) {
            28.0
        } else {
            82.0
        };
        let [r, g, b] = tonal_rgb(tone, 18.0, hue);
        format!("@define-color md_stage mix(@md_surface_low, #{r:02x}{g:02x}{b:02x}, 0.22);")
    } else {
        String::new()
    };
    if let Err(error) = theme.tint.load_from_data(css.as_bytes()) {
        eprintln!("Не удалось загрузить цвет обложки: {error}");
    }
}

pub(crate) fn cover_hue(cover: &gtk::gdk_pixbuf::Pixbuf) -> Option<f64> {
    let bytes = cover.read_pixel_bytes();
    let channels = cover.n_channels() as usize;
    let mut bins = [(0.0, 0.0, 0.0); 24];
    for row in bytes.as_ref().chunks(cover.rowstride() as usize) {
        for pixel in row[..cover.width() as usize * channels].chunks_exact(channels) {
            if channels == 4 && pixel[3] < 128 {
                continue;
            }
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]].map(|v| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            });
            let x = ((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047).cbrt();
            let y = (0.2126729 * r + 0.7151522 * g + 0.0721750 * b).cbrt();
            let z = ((0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883).cbrt();
            let a = 500.0 * (x - y);
            let b = 200.0 * (y - z);
            let chroma = a.hypot(b);
            if chroma < 8.0 || y < 0.24 {
                continue;
            }
            let hue = a.atan2(b).rem_euclid(std::f64::consts::TAU);
            let index = (hue * 24.0 / std::f64::consts::TAU).round() as usize % 24;
            let bin = &mut bins[index];
            let weight = chroma.min(50.0).sqrt();
            bin.0 += weight;
            bin.1 += hue.sin() * weight;
            bin.2 += hue.cos() * weight;
        }
    }
    let (weight, sin, cos) = bins.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    (weight > 0.0).then(|| sin.atan2(cos))
}

fn tonal_rgb(tone: f64, chroma: f64, hue: f64) -> [u8; 3] {
    let y = (tone + 16.0) / 116.0;
    let linear = |v: f64| {
        if v > 6.0 / 29.0 {
            v.powi(3)
        } else {
            (116.0 * v - 16.0) / 903.3
        }
    };
    let mut chroma = chroma;
    loop {
        let x = 0.95047 * linear(y + chroma * hue.sin() / 500.0);
        let z = 1.08883 * linear(y - chroma * hue.cos() / 200.0);
        let y = linear(y);
        let rgb = [
            3.2404542 * x - 1.5371385 * y - 0.4985314 * z,
            -0.9692660 * x + 1.8760108 * y + 0.0415560 * z,
            0.0556434 * x - 0.2040259 * y + 1.0572252 * z,
        ];
        if chroma < 0.5 || rgb.iter().all(|v| (0.0..=1.0).contains(v)) {
            return rgb.map(|v| {
                let v = v.clamp(0.0, 1.0);
                let srgb = if v <= 0.0031308 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (srgb * 255.0).round() as u8
            });
        }
        chroma *= 0.8;
    }
}
