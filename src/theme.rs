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
@define-color md_primary #8fd1dd;
@define-color md_on_primary #00363f;
@define-color md_primary_container #004e5a;
@define-color md_on_primary_container #aaedf9;
@define-color md_secondary_container #334a50;
@define-color md_on_secondary_container #cde7ed;
@define-color md_tertiary_container #3d4565;
@define-color md_on_tertiary_container #dce2ff;
@define-color md_surface #0f1517;
@define-color md_surface_low #171e20;
@define-color md_surface_container #1b2325;
@define-color md_surface_high #252d2f;
@define-color md_on_surface #dce4e6;
@define-color md_on_surface_variant #bdc9cc;
@define-color md_outline #879497;
@define-color md_outline_variant #3e484b;
@define-color md_inverse_surface #dce4e6;
@define-color md_inverse_on_surface #2b3234;
@define-color md_error #ffb4ab;
";

thread_local! {
    static THEME: std::cell::OnceCell<Theme> = const { std::cell::OnceCell::new() };
}

struct Theme {
    provider: gtk::CssProvider,
    settings: gtk::Settings,
    hue: std::cell::Cell<Option<f64>>,
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
            settings.connect_gtk_application_prefer_dark_theme_notify(|_| refresh());
            Theme {
                provider,
                settings,
                hue: std::cell::Cell::new(None),
            }
        });
    });
    refresh();
}

pub(crate) fn set_hue(hue: Option<f64>) {
    let changed = THEME.with(|theme| {
        theme
            .get()
            .is_some_and(|theme| theme.hue.replace(hue) != hue)
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

fn refresh() {
    THEME.with(|theme| {
        let Some(theme) = theme.get() else { return };
        let gtk_theme = std::env::var("GTK_THEME").unwrap_or_default();
        let dark = match gtk_theme.rsplit_once(':').map(|(_, variant)| variant) {
            Some("dark") => true,
            Some("light") => false,
            _ => {
                theme.settings.is_gtk_application_prefer_dark_theme()
                    || theme
                        .settings
                        .gtk_theme_name()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("dark"))
            }
        };
        let baseline = if dark { DARK } else { LIGHT };
        let dynamic = theme
            .hue
            .get()
            .map_or_else(String::new, |hue| palette(hue, dark));
        let css = format!("{baseline}{dynamic}{}", include_str!("style.css"));
        if let Err(error) = theme.provider.load_from_data(css.as_bytes()) {
            eprintln!("Не удалось загрузить стиль: {error}");
        }
    });
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

fn palette(hue: f64, dark: bool) -> String {
    use std::fmt::Write;
    let mut css = String::new();
    // Fixed CIELAB lightness preserves the luminance relationship between paired roles.
    for (role, light, night, chroma, shift) in [
        ("primary", 39., 82., 45., 0.),
        ("on_primary", 100., 19., 36., 0.),
        ("primary_container", 89., 31., 30., 0.),
        ("on_primary_container", 12., 93., 26., 0.),
        ("secondary_container", 88., 30., 17., 0.),
        ("on_secondary_container", 13., 91., 17., 0.),
        ("tertiary_container", 89., 32., 32., 55.),
        ("on_tertiary_container", 12., 92., 26., 55.),
        ("surface", 98., 7., 4., 0.),
        ("surface_low", 95., 11., 5., 0.),
        ("surface_container", 92., 15., 6., 0.),
        ("surface_high", 87., 20., 8., 0.),
        ("on_surface", 10., 92., 4., 0.),
        ("on_surface_variant", 28., 78., 10., 0.),
        ("outline", 48., 63., 9., 0.),
        ("outline_variant", 78., 31., 8., 0.),
        ("inverse_surface", 20., 92., 5., 0.),
        ("inverse_on_surface", 96., 17., 5., 0.),
    ] {
        let tone = if dark { night } else { light };
        let angle = hue + f64::to_radians(shift);
        let [r, g, b] = tonal_rgb(tone, chroma, angle);
        let _ = writeln!(css, "@define-color md_{role} #{r:02x}{g:02x}{b:02x};");
    }
    css
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
