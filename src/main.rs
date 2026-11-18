#[cfg(test)]
use gtk::gdk_pixbuf::Colorspace;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

mod artwork;
mod cover_art;
mod library;
mod lyrics;
mod motion;
mod mpv;
mod now_playing;
mod playback_controls;
mod session;
mod shuffle;
mod theme;
mod track_menu;
mod ui_updates;
mod yandex_export;
use library::{collect_audio, extract_cover, preview_track, read_track};
use lyrics::online_lyrics;
use mpv::Mpv;
use session::{Appearance, Session};
use shuffle::ShuffleOrder;
use ui_updates::UiUpdates;

const PAGE_NAMES: [&str; 3] = ["Плеер", "Очередь", "Настройки"];

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    album: String,
    duration: f64,
    cover_path: Option<PathBuf>,
    lyrics: Vec<Lyric>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Lyric {
    time: Option<f64>,
    text: String,
}

struct Model {
    updates: Rc<UiUpdates>,
    tracks: Vec<Track>,
    current: Option<usize>,
    library_revision: u64,
    queue_metadata_changed: std::collections::BTreeSet<usize>,
    cover_pending: std::collections::HashSet<PathBuf>,
    cover_attempted: std::collections::HashSet<PathBuf>,
    metadata_ready: std::collections::HashSet<PathBuf>,
    metadata_pending: std::collections::HashSet<PathBuf>,
    lyrics_pending: std::collections::HashSet<PathBuf>,
    lyrics_results: std::collections::HashMap<PathBuf, String>,
    shuffle: bool,
    shuffle_order: ShuffleOrder,
    play_next: Option<PathBuf>,
    repeat: usize,
    online_covers: bool,
    volume: f64,
    muted: bool,
    appearance: Appearance,
    speed: f64,
    status: Option<String>,
    imports_pending: usize,
    failed_tracks: std::collections::HashSet<usize>,
    mpv: Option<Mpv>,
}

fn request_ui(model: &Rc<RefCell<Model>>) {
    model.borrow().updates.request();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ym-export") {
        if let Err(error) = yandex_export::run(&args[1..]) {
            eprintln!("Не удалось экспортировать плейлист: {error}");
            std::process::exit(1);
        }
        return;
    }
    let app = gtk::Application::new(Some("local.ownplayer.desktop"), Default::default());
    app.connect_startup(|_| {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(true);
        }
        if let Some(screen) = gtk::gdk::Screen::default() {
            theme::install(&screen);
        }
    });
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &gtk::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let model = Rc::new(RefCell::new(Model {
        updates: Rc::default(),
        tracks: Vec::new(),
        current: None,
        library_revision: 0,
        queue_metadata_changed: Default::default(),
        cover_pending: std::collections::HashSet::new(),
        cover_attempted: Default::default(),
        metadata_ready: Default::default(),
        metadata_pending: Default::default(),
        lyrics_pending: Default::default(),
        lyrics_results: Default::default(),
        shuffle: false,
        shuffle_order: ShuffleOrder::default(),
        play_next: None,
        repeat: 0,
        online_covers: true,
        volume: 75.0,
        muted: false,
        appearance: Appearance::default(),
        speed: 1.0,
        status: None,
        imports_pending: 0,
        failed_tracks: Default::default(),
        mpv: None,
    }));
    restore_session(&model);
    let appearance = model.borrow().appearance;
    theme::set_hue(None);
    theme::set_dynamic(appearance.dynamic_background);
    set_decorative_motion(appearance.animations);
    let window = gtk::ApplicationWindow::new(app);
    window.set_title("Veyra");
    window.set_default_size(1160, 760);
    window.set_size_request(640, 460);
    window.set_position(gtk::WindowPosition::Center);

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    root.set_border_width(12);

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    sidebar.set_size_request(72, -1);
    sidebar.style_context().add_class("sidebar");

    let brand = gtk::Box::new(gtk::Orientation::Vertical, 0);
    brand.set_halign(gtk::Align::Center);
    brand.set_size_request(40, 40);
    brand.set_margin_bottom(12);
    brand.style_context().add_class("brand-mark");
    let brand_icon = symbolize("audio-x-generic-symbolic", 20);
    brand_icon.style_context().add_class("brand-icon");
    brand.set_center_widget(Some(&brand_icon));
    sidebar.pack_start(&brand, false, false, 0);

    let pages = gtk::Stack::new();
    pages.set_hhomogeneous(false);
    pages.set_vhomogeneous(false);
    pages.set_transition_type(gtk::StackTransitionType::Crossfade);
    pages.set_transition_duration(180);
    let page_buttons: Rc<RefCell<Vec<gtk::Button>>> = Rc::new(RefCell::new(Vec::new()));
    for (index, name) in PAGE_NAMES[..2].iter().enumerate() {
        let name = name.to_string();
        let icon = if index == 0 {
            "audio-x-generic-symbolic"
        } else {
            "view-list-symbolic"
        };
        let button = rail_button(icon, &name, &name, "nav-item");
        let pages_clone = pages.clone();
        let all_buttons = Rc::downgrade(&page_buttons);
        button.connect_clicked(move |_| {
            if let Some(all_buttons) = all_buttons.upgrade() {
                set_page(&pages_clone, &all_buttons, index);
            }
        });
        sidebar.pack_start(&button, false, false, 0);
        page_buttons.borrow_mut().push(button);
    }
    if let Some(first) = page_buttons.borrow().first() {
        first.style_context().add_class("selected");
    }
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.pack_start(&spacer, true, true, 0);
    let add_files = rail_button(
        "list-add-symbolic",
        "Файлы",
        "Добавить файлы",
        "sidebar-action",
    );
    let add_folder = rail_button(
        "folder-open-symbolic",
        "Папка",
        "Добавить папку",
        "sidebar-action",
    );
    let settings = rail_button(
        "preferences-system-symbolic",
        "Настройки",
        "Настройки",
        "sidebar-action",
    );
    sidebar.pack_start(&add_files, false, false, 0);
    sidebar.pack_start(&add_folder, false, false, 0);
    let sidebar_divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    sidebar_divider.set_size_request(-1, 1);
    sidebar_divider.set_margin_top(8);
    sidebar_divider.set_margin_bottom(8);
    sidebar_divider.style_context().add_class("divider");
    sidebar.pack_start(&sidebar_divider, false, false, 0);
    sidebar.pack_start(&settings, false, false, 0);
    page_buttons.borrow_mut().push(settings.clone());

    apply_sidebar_mode(&sidebar, appearance.expanded_sidebar);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::Box::new(gtk::Orientation::Vertical, 3);
    header.set_margin_start(8);
    let subheading = gtk::Label::new(None);
    subheading.set_xalign(0.0);
    subheading.set_line_wrap(true);
    subheading.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    subheading.set_max_width_chars(72);
    subheading.set_no_show_all(true);
    subheading.style_context().add_class("status-banner");
    header.pack_start(&subheading, false, false, 0);
    subheading.set_margin_bottom(10);
    content.pack_start(&header, false, false, 0);

    let (player_page, _) = now_playing::build(&model);
    let (queue_page, queue_search) = build_queue_page(&model);
    pages.add_named(&player_page, "Плеер");
    pages.add_named(&queue_page, "Очередь");
    pages.add_named(&build_settings_page(&model, &sidebar), "Настройки");
    content.pack_start(&pages, true, true, 0);
    let controls = playback_controls::build(&model);

    let main_column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    main_column.pack_start(&content, true, true, 0);
    main_column.pack_start(&controls, false, false, 0);
    root.pack_start(&sidebar, false, false, 0);
    root.pack_start(&main_column, true, true, 0);
    window.add(&root);

    let model_files = model.clone();
    add_files.connect_clicked(move |_| {
        if let Some(files) = choose_files(false) {
            append_paths(&model_files, files);
        }
    });
    let model_folder = model.clone();
    add_folder.connect_clicked(move |_| {
        if let Some(files) = choose_files(true) {
            append_paths(&model_folder, files);
        }
    });
    let settings_pages = pages.downgrade();
    let settings_buttons = Rc::downgrade(&page_buttons);
    let previous_page = Rc::new(Cell::new(0));
    let previous = previous_page.clone();
    settings.connect_clicked(move |_| {
        if let (Some(pages), Some(buttons)) = (settings_pages.upgrade(), settings_buttons.upgrade())
        {
            let current = pages.visible_child_name();
            if current.as_deref() == Some(PAGE_NAMES[2]) {
                set_page(&pages, &buttons, previous.get());
            } else {
                previous.set(usize::from(current.as_deref() == Some(PAGE_NAMES[1])));
                set_page(&pages, &buttons, 2);
            }
        }
    });
    let settings_pages = pages.downgrade();
    let settings_buttons = Rc::downgrade(&page_buttons);
    let keyboard_model = model.clone();
    window.connect_key_press_event(move |window, event| {
        let key = event.keyval();
        let modifiers = event.state() & gtk::accelerator_get_default_mod_mask();
        if key == gtk::gdk::keys::constants::f && modifiers == gtk::gdk::ModifierType::CONTROL_MASK
        {
            if let (Some(pages), Some(buttons)) =
                (settings_pages.upgrade(), settings_buttons.upgrade())
            {
                set_page(&pages, &buttons, 1);
                queue_search.grab_focus();
                return glib::Propagation::Stop;
            }
        }
        if key == gtk::gdk::keys::constants::Escape {
            if let (Some(pages), Some(buttons)) =
                (settings_pages.upgrade(), settings_buttons.upgrade())
            {
                if pages.visible_child_name().as_deref() == Some(PAGE_NAMES[2]) {
                    set_page(&pages, &buttons, previous_page.get());
                    return glib::Propagation::Stop;
                }
            }
        }
        if playback_controls::shortcut(window, event, &keyboard_model) {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    let model_close = model.clone();
    window.connect_delete_event(move |_, _| {
        save_session(&model_close);
        session::flush();
        glib::Propagation::Proceed
    });

    let buttons_lifetime = page_buttons.clone();
    window.connect_destroy(move |_| buttons_lifetime.borrow_mut().clear());
    let model_count = model.clone();
    let subheading_update = subheading.clone();
    let updates = model.borrow().updates.clone();
    updates.listen(&window, move || {
        let state = model_count.borrow();
        let text = if let Some(message) = &state.status {
            message.as_str()
        } else if state.imports_pending > 0 {
            "Добавляем музыку…"
        } else {
            ""
        };
        if subheading_update.text() != text {
            subheading_update.set_text(text);
            subheading_update.set_visible(!text.is_empty());
        }
    });
    updates.listen(&window, {
        let model = Rc::downgrade(&model);
        move || {
            if let Some(model) = model.upgrade() {
                arm_mpv_writes(&model);
            }
        }
    });

    start_mpv(&model);

    sync_stack_motion(window.upcast_ref());
    motion::animate_buttons(window.upcast_ref());
    window.show_all();
    window.set_focus(gtk::Widget::NONE);
}

fn symbolize(name: &str, size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(Some(name), gtk::IconSize::Button);
    image.set_pixel_size(size);
    image
}

fn icon_button(icon: &str, tooltip: &str, class: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_image(Some(&symbolize(icon, 18)));
    describe_control(&button, tooltip);
    button.style_context().add_class(class);
    button
}

fn describe_control(widget: &impl IsA<gtk::Widget>, label: &str) {
    widget.set_tooltip_text(Some(label));
    if let Some(accessible) = widget.accessible() {
        accessible.set_name(label);
    }
}

fn rail_button(icon: &str, label: &str, tooltip: &str, class: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.style_context().add_class(class);
    button.set_focus_on_click(false);
    describe_control(&button, tooltip);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_halign(gtk::Align::Center);
    column.set_valign(gtk::Align::Center);
    column.pack_start(&symbolize(icon, 22), false, false, 0);
    column.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
    button.add(&column);
    button
}

fn apply_sidebar_mode(sidebar: &gtk::Box, expanded: bool) {
    sidebar.set_width_request(if expanded { 156 } else { 72 });
    set_style_class(sidebar, "expanded", expanded);
    for button in sidebar
        .children()
        .into_iter()
        .filter_map(|child| child.downcast::<gtk::Button>().ok())
    {
        if let Some(content) = button
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        {
            content.set_orientation(if expanded {
                gtk::Orientation::Horizontal
            } else {
                gtk::Orientation::Vertical
            });
            content.set_spacing(if expanded { 12 } else { 4 });
            content.set_halign(if expanded {
                gtk::Align::Start
            } else {
                gtk::Align::Center
            });
        }
    }
}

fn cover_placeholder(size: i32, glyph: i32) -> gtk::Box {
    let area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    area.set_size_request(size, size);
    area.style_context().add_class("cover-placeholder");
    let icon = symbolize("audio-x-generic-symbolic", glyph);
    icon.style_context().add_class("cover-glyph");
    area.set_center_widget(Some(&icon));
    area
}

fn tracks_phrase(count: usize) -> String {
    let word = match count % 10 {
        1 if count % 100 != 11 => "трек",
        2..=4 if !(12..=14).contains(&(count % 100)) => "трека",
        _ => "треков",
    };
    format!("{count} {word}")
}

fn set_page(pages: &gtk::Stack, buttons: &Rc<RefCell<Vec<gtk::Button>>>, index: usize) {
    pages.set_visible_child_name(PAGE_NAMES[index]);
    for (position, button) in buttons.borrow().iter().enumerate() {
        if position == index {
            button.style_context().add_class("selected");
        } else {
            button.style_context().remove_class("selected");
        }
    }
}

// Keep the visible image until the selected track's cover lookup and decode settle.
fn current_artwork(
    track: &Track,
    rendered: &mut Option<(Option<PathBuf>, Option<PathBuf>)>,
    state: &Model,
) -> Option<Option<Rc<artwork::Artwork>>> {
    let missing_cover_track = track.cover_path.is_none().then_some(&track.path);
    if rendered
        .as_ref()
        .map(|(cover, track)| (cover.as_ref(), track.as_ref()))
        == Some((track.cover_path.as_ref(), missing_cover_track))
        || (missing_cover_track.is_some() && state.cover_pending.contains(&track.path))
    {
        return None;
    }
    let image = artwork::get(track.cover_path.as_deref(), true, &state.updates)?;
    *rendered = Some((track.cover_path.clone(), missing_cover_track.cloned()));
    Some(image)
}

fn crossfade_cover(
    stack: &gtk::Stack,
    image: &gtk::Image,
    alternate: &gtk::Image,
    pixbuf: Option<&Pixbuf>,
) {
    let Some(pixbuf) = pixbuf else {
        stack.set_visible_child_name("placeholder");
        return;
    };
    let (image, name) = if stack.visible_child_name().as_deref() == Some("image") {
        (alternate, "image-alt")
    } else {
        (image, "image")
    };
    image.set_from_pixbuf(Some(pixbuf));
    stack.set_visible_child_name(name);
}

fn build_lyrics_view(model: &Rc<RefCell<Model>>, views: &gtk::Stack) -> gtk::Widget {
    let overlay = gtk::Overlay::new();
    overlay.style_context().add_class("lyrics-overlay");

    let backdrop = gtk::DrawingArea::new();
    backdrop.style_context().add_class("lyrics-backdrop");
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    let backdrop_source: Rc<RefCell<Option<Pixbuf>>> = Rc::new(RefCell::new(None));
    let backdrop_previous: Rc<RefCell<Option<Pixbuf>>> = Rc::new(RefCell::new(None));
    let backdrop_mix = motion::Tween::new(1.0);
    {
        let source = backdrop_source.clone();
        let previous = backdrop_previous.clone();
        let mix = backdrop_mix.clone();
        backdrop.connect_draw(move |area, cr| {
            let allocation = area.allocation();
            let width = allocation.width() as f64;
            let height = allocation.height() as f64;
            let radius = 28.0;
            let half_pi = std::f64::consts::FRAC_PI_2;
            cr.new_sub_path();
            cr.arc(width - radius, radius, radius, -half_pi, 0.0);
            cr.arc(width - radius, height - radius, radius, 0.0, half_pi);
            cr.arc(radius, height - radius, radius, half_pi, 2.0 * half_pi);
            cr.arc(radius, radius, radius, 2.0 * half_pi, 3.0 * half_pi);
            cr.close_path();
            cr.clip();
            gtk::render_background(&area.style_context(), cr, 0.0, 0.0, width, height);
            for (image, opacity) in [
                (previous.borrow(), 1.0 - mix.value()),
                (source.borrow(), mix.value()),
            ] {
                let Some(pixbuf) = image.as_ref().filter(|_| opacity > 0.0) else {
                    continue;
                };
                let source_width = f64::from(pixbuf.width());
                let source_height = f64::from(pixbuf.height());
                let scale = (width / source_width).max(height / source_height);
                let _ = cr.save();
                cr.translate(
                    (width - source_width * scale) / 2.0,
                    (height - source_height * scale) / 2.0,
                );
                cr.scale(scale, scale);
                cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
                let _ = cr.paint_with_alpha(opacity);
                let _ = cr.restore();
            }
            glib::Propagation::Proceed
        });
    }
    overlay.add(&backdrop);

    let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    scrim.style_context().add_class("lyrics-scrim");
    overlay.add_overlay(&scrim);
    overlay.set_overlay_pass_through(&scrim, true);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_margin_top(16);
    page.set_margin_bottom(16);
    page.set_margin_start(28);
    page.set_margin_end(28);
    overlay.add_overlay(&page);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let back = icon_button("go-previous-symbolic", "К плееру", "icon-button");
    back.style_context().add_class("lyrics-back");
    back.set_valign(gtk::Align::Center);
    back.set_focus_on_click(false);
    header.pack_start(&back, false, false, 0);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_valign(gtk::Align::Center);
    let lyrics_title = gtk::Label::new(Some("Текст песни"));
    lyrics_title.set_xalign(0.0);
    lyrics_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    lyrics_title.set_max_width_chars(32);
    lyrics_title.style_context().add_class("lyrics-title");
    let lyrics_artist = gtk::Label::new(Some("Выбери трек"));
    lyrics_artist.set_xalign(0.0);
    lyrics_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    lyrics_artist.set_max_width_chars(32);
    lyrics_artist.style_context().add_class("lyrics-artist");
    titles.pack_start(&lyrics_title, false, false, 0);
    titles.pack_start(&lyrics_artist, false, false, 0);
    header.pack_start(&titles, true, true, 0);
    let find = gtk::Button::new();
    find.style_context().add_class("tonal-button");
    let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    find_row.pack_start(&symbolize("system-search-symbolic", 16), false, false, 0);
    let find_label = gtk::Label::new(Some("Найти текст"));
    find_row.pack_start(&find_label, false, false, 0);
    find.add(&find_row);
    find.show_all();
    find.set_no_show_all(true);
    header.pack_start(&find, false, false, 0);
    page.pack_start(&header, false, false, 0);

    let body = gtk::Stack::new();
    body.set_transition_type(gtk::StackTransitionType::Crossfade);
    body.set_transition_duration(200);
    let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    empty_box.set_halign(gtk::Align::Center);
    empty_box.set_valign(gtk::Align::Center);
    let empty_icon = symbolize("format-justify-left-symbolic", 40);
    empty_icon.style_context().add_class("empty-glyph");
    empty_box.pack_start(&empty_icon, false, false, 0);
    let empty = gtk::Label::new(Some("Выбери трек, чтобы увидеть текст"));
    empty.set_line_wrap(true);
    empty.set_justify(gtk::Justification::Center);
    empty.set_max_width_chars(46);
    empty.style_context().add_class("empty-state");
    empty_box.pack_start(&empty, false, false, 0);
    body.add_named(&empty_box, "empty");

    let lyrics_list = gtk::ListBox::new();
    lyrics_list.set_selection_mode(gtk::SelectionMode::Single);
    lyrics_list.set_activate_on_single_click(false);
    lyrics_list.style_context().add_class("lyrics-list");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let spacer_top = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer_top.set_size_request(-1, 0);
    let spacer_bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer_bottom.set_size_request(-1, 0);
    content.pack_start(&spacer_top, false, false, 0);
    content.pack_start(&lyrics_list, false, false, 0);
    content.pack_start(&spacer_bottom, false, false, 0);
    let lyrics_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    lyrics_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    lyrics_scroll.set_overlay_scrolling(false);
    lyrics_scroll.set_shadow_type(gtk::ShadowType::None);
    lyrics_scroll.style_context().add_class("lyrics-scroll");
    lyrics_scroll.add(&content);

    body.add_named(&lyrics_scroll, "list");
    body.set_visible_child_name("empty");
    page.pack_start(&body, true, true, 0);

    let hint = gtk::Label::new(Some("Текст песни"));
    hint.set_xalign(0.5);
    hint.set_line_wrap(true);
    hint.set_justify(gtk::Justification::Center);
    hint.style_context().add_class("lyrics-hint");
    page.pack_start(&hint, false, false, 0);

    let views_back = views.downgrade();
    back.connect_clicked(move |_| {
        if let Some(views) = views_back.upgrade() {
            views.set_visible_child_name("artwork");
        }
    });

    let model_find = model.clone();
    find.connect_clicked(move |_| start_lyrics_job(&model_find, true));
    let updates = model.borrow().updates.clone();
    let browsing_until = Rc::new(Cell::new(Instant::now()));
    let seek_browsing = browsing_until.clone();
    let explicit_seek = Rc::new(Cell::new(false));
    let row_seek = explicit_seek.clone();
    let model_play = model.clone();
    lyrics_list.connect_row_activated(move |list, row| {
        let line_index = row.index() as usize;
        let mut state = model_play.borrow_mut();
        let time = state
            .current
            .and_then(|i| state.tracks.get(i))
            .and_then(|t| t.lyrics.get(line_index))
            .and_then(|line| line.time);
        if let (Some(time), Some(player)) = (time, state.mpv.as_mut()) {
            if playback_controls::seek_absolute(player, time) {
                seek_browsing.set(Instant::now());
                row_seek.set(true);
            }
        }
        state.updates.request();
        list.select_row(None::<&gtk::ListBoxRow>);
    });
    let resume = gtk::Button::with_label("К текущей строке");
    resume.style_context().add_class("lyrics-resume");
    resume.set_halign(gtk::Align::Center);
    resume.show_all();
    resume.set_no_show_all(true);
    resume.hide();
    resume.set_valign(gtk::Align::End);
    resume.set_margin_bottom(48);
    overlay.add_overlay(&resume);
    let follow = motion::Tween::new(0.0);
    let adjustment = lyrics_scroll.vadjustment();
    let unmap_follow = follow.clone();
    lyrics_scroll.connect_unmap(move |scroll| unmap_follow.snap(scroll.vadjustment().value()));
    let scrollbar_drag = Rc::new(Cell::new(false));
    let browse = {
        let browsing = browsing_until.clone();
        let motion = follow.clone();
        let adjustment = adjustment.clone();
        let updates = updates.clone();
        let timeout = Rc::new(RefCell::new(None::<glib::SourceId>));
        let active_timeout = timeout.clone();
        lyrics_scroll.connect_destroy(move |_| {
            if let Some(source) = active_timeout.take() {
                source.remove();
            }
        });
        move || {
            let cooldown = Duration::from_secs(5);
            browsing.set(Instant::now() + cooldown);
            motion.snap(adjustment.value());
            updates.request();
            if let Some(source) = timeout.take() {
                source.remove();
            }
            let pending = timeout.clone();
            let updates = Rc::downgrade(&updates);
            timeout.replace(Some(glib::timeout_add_local_once(cooldown, move || {
                pending.take();
                if let Some(updates) = updates.upgrade() {
                    updates.request();
                }
            })));
        }
    };
    let wheel_browse = browse.clone();
    lyrics_scroll.connect_scroll_event(move |_, _| {
        wheel_browse();
        glib::Propagation::Proceed
    });
    if let Some(scrollbar) = lyrics_scroll.vscrollbar() {
        let press_browse = browse.clone();
        let dragging = scrollbar_drag.clone();
        scrollbar.connect_button_press_event(move |_, _| {
            dragging.set(true);
            press_browse();
            glib::Propagation::Proceed
        });
        let dragging = scrollbar_drag.clone();
        scrollbar.connect_button_release_event(move |_, _| {
            dragging.set(false);
            browse();
            glib::Propagation::Proceed
        });
    }
    let browsing = browsing_until.clone();
    resume.connect_clicked({
        let updates = updates.clone();
        move |_| {
            browsing.set(Instant::now());
            updates.request();
        }
    });
    adjustment.connect_changed({
        let updates = updates.clone();
        move |_| updates.request()
    });
    lyrics_scroll.connect_size_allocate({
        let updates = updates.clone();
        move |_, _| updates.request()
    });

    let model_update = model.clone();
    let list_update = lyrics_list.clone();
    let lyrics_adjustment = lyrics_scroll.vadjustment();
    let label_update = empty.clone();
    let body_update = body.clone();
    let hint_update = hint.clone();
    let title_update = lyrics_title.clone();
    let artist_update = lyrics_artist.clone();
    let backdrop_update = backdrop.clone();
    let backdrop_source_update = backdrop_source.clone();

    let lyrics_view = overlay.downgrade();
    let mut rendered_track: Option<PathBuf> = None;
    let mut rendered_cover = None;
    let mut rendered_lines: Vec<Lyric> = Vec::new();
    let mut rendered_labels = None;
    let mut focus_style: Option<(Option<usize>, bool)> = None;
    let mut dynamic_background = model.borrow().appearance.dynamic_background;
    let mut synchronized = false;
    let mut active = None;
    let mut lyric_times = Vec::new();
    let mut last_position: Option<f64> = None;
    let mut snap_follow = false;
    let mut rendered_status: Option<(bool, bool, bool, Option<String>)> = None;

    updates.listen(&overlay, move || {
        if !lyrics_view.upgrade().is_some_and(|view| view.is_mapped()) {
            return;
        }
        let state = model_update.borrow();
        if dynamic_background != state.appearance.dynamic_background {
            dynamic_background = state.appearance.dynamic_background;
            rendered_cover = None;
            backdrop_source_update.replace(None);
            backdrop_previous.replace(None);
            backdrop_mix.snap(1.0);
            backdrop_update.queue_draw();
        }
        if let Some(index) = state.current {
            if let Some(track) = state.tracks.get(index) {
                let pending = state.lyrics_pending.contains(&track.path);
                let ready = state.metadata_ready.contains(&track.path);
                let status = (
                    track.lyrics.is_empty(),
                    pending,
                    ready,
                    state.lyrics_results.get(&track.path).map(String::as_str),
                );
                if rendered_status
                    .as_ref()
                    .map(|(empty, pending, ready, result)| {
                        (*empty, *pending, *ready, result.as_deref())
                    })
                    != Some(status)
                {
                    rendered_status = Some((
                        status.0,
                        status.1,
                        status.2,
                        status.3.map(ToOwned::to_owned),
                    ));
                    find.set_visible(status.0);
                    find.set_sensitive(ready && !pending);
                    find_label.set_text(if pending {
                        "Ищем текст…"
                    } else if status.3.is_some() {
                        "Искать снова"
                    } else {
                        "Найти текст"
                    });
                    if status.0 {
                        label_update.set_text(if pending {
                            "Ищем текст песни"
                        } else if !ready {
                            "Читаем данные трека…"
                        } else {
                            status.3.unwrap_or("Готовим поиск текста…")
                        });
                        hint_update.set_text("Текст ищется автоматически в LRCLIB. Можно добавить .lrc рядом с аудиофайлом.");
                    }
                    body_update.set_visible_child_name(if status.0 { "empty" } else { "list" });
                }
                if let Some(image) = dynamic_background
                    .then(|| current_artwork(track, &mut rendered_cover, &state))
                    .flatten()
                {
                    let pixbuf = image.and_then(|image| image.backdrop.clone());
                    let previous = backdrop_source_update.replace(pixbuf);
                    backdrop_previous.replace(previous);
                    backdrop_mix.snap(0.0);
                    let previous = backdrop_previous.clone();
                    backdrop_mix.animate(&backdrop_update, 1.0, 180, move |area, value| {
                        if value == 1.0 {
                            previous.replace(None);
                        }
                        area.queue_draw();
                    });
                    backdrop_update.queue_draw();
                }
                let labels = (&track.title, &track.artist);
                if rendered_labels
                    .as_ref()
                    .map(|(title, artist)| (title, artist))
                    != Some(labels)
                {
                    title_update.set_text(labels.0);
                    artist_update.set_text(labels.1);
                    rendered_labels = Some((track.title.clone(), track.artist.clone()));
                }
                if rendered_track.as_ref() != Some(&track.path) {
                    rendered_track = Some(track.path.clone());
                    browsing_until.set(Instant::now());
                    scrollbar_drag.set(false);
                    last_position = None;
                }
                if rendered_lines != track.lyrics {
                    rendered_lines.clone_from(&track.lyrics);
                    last_position = None;
                    focus_style = None;

                    lyric_times.clear();
                    lyric_times.extend(
                        rendered_lines
                            .iter()
                            .enumerate()
                            .filter_map(|(index, line)| line.time.map(|time| (time, index))),
                    );
                    lyric_times.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    synchronized = !lyric_times.is_empty();
                    hint_update.set_text(if synchronized {
                        "Двойной щелчок по строке — перейти к этому моменту"
                    } else if track.lyrics.is_empty() {
                        "Текст ищется автоматически в LRCLIB. Можно добавить .lrc рядом с аудиофайлом."
                    } else {
                        "Этот текст без временных меток — листай в своём темпе."
                    });
                    for child in list_update.children() {
                        list_update.remove(&child);
                    }
                    for line in &track.lyrics {
                        let row = gtk::ListBoxRow::new();
                        row.style_context().add_class("lyric-row");
                        let text = gtk::Label::new(Some(&line.text));
                        text.set_xalign(0.0);
                        text.set_line_wrap(true);
                        text.set_justify(gtk::Justification::Left);
                        text.set_margin_top(12);
                        text.set_margin_bottom(12);
                        text.set_margin_start(24);
                        text.set_margin_end(24);
                        text.style_context().add_class("lyric-text");
                        if !synchronized {
                            text.style_context().add_class("lyric-plain");
                        }
                        row.add(&text);
                        list_update.add(&row);
                    }
                    list_update.show_all();
                    follow.snap(0.0);
                    lyrics_adjustment.set_value(0.0);
                }
                if synchronized && !track.lyrics.is_empty() && lyrics_scroll.is_mapped() {
                    let position = state
                        .mpv
                        .as_ref()
                        .map_or(0.0, |player| player.position(state.speed));
                    snap_follow |= explicit_seek.replace(false)
                        || last_position.is_some_and(|last| (last - position).abs() > 0.75);
                    if last_position != Some(position) {
                        last_position = Some(position);
                        let end =
                            lyric_times.partition_point(|(time, _)| *time <= position + 0.020);
                        active = end.checked_sub(1).map(|i| lyric_times[i].1);
                    }
                    let browsing = Instant::now() < browsing_until.get()
                        || scrollbar_drag.get()
                        || lyrics_scroll.has_grab();
                    resume.set_visible(browsing);
                    if focus_style != Some((active, browsing)) {
                        update_lyric_focus(
                            &list_update,
                            focus_style,
                            (active, browsing),
                            rendered_lines.len(),
                        );
                        focus_style = Some((active, browsing));
                    }
                    let height = lyrics_adjustment.page_size();
                    let reading_line = height * 0.32;
                    let row_height = |index| {
                        list_update
                            .row_at_index(index)
                            .map_or(0.0, |row| f64::from(row.allocated_height()))
                    };
                    let mut relayout = false;
                    for (spacer, padding) in [
                        (&spacer_top, reading_line - row_height(0) / 2.0),
                        (
                            &spacer_bottom,
                            height
                                - reading_line
                                - row_height(rendered_lines.len() as i32 - 1) / 2.0,
                        ),
                    ] {
                        let padding = padding.ceil().max(0.0) as i32;
                        if spacer.height_request() != padding {
                            spacer.set_size_request(-1, padding);
                            relayout = true;
                        }
                    }
                    if browsing {
                        follow.snap(lyrics_adjustment.value());
                    } else if !relayout {
                        let goal = active
                            .and_then(|i| list_update.row_at_index(i as i32))
                            .and_then(|row| {
                                let (_, y) = row.translate_coordinates(&content, 0, 0)?;
                                Some(
                                    f64::from(y) + f64::from(row.allocated_height()) / 2.0
                                        - reading_line,
                                )
                            })
                            .unwrap_or(0.0)
                            .clamp(0.0, (lyrics_adjustment.upper() - height).max(0.0));
                        if std::mem::take(&mut snap_follow) {
                            follow.snap(goal);
                            lyrics_adjustment.set_value(goal);
                        } else if (goal - lyrics_adjustment.value()).abs() > 0.5 {
                            follow.animate(&lyrics_scroll, goal, 200, |scroll, value| {
                                scroll.vadjustment().set_value(value);
                            });
                        }
                    }
                } else {
                    resume.hide();
                    for spacer in [&spacer_top, &spacer_bottom] {
                        if spacer.height_request() != 0 {
                            spacer.set_size_request(-1, 0);
                        }
                    }
                }
            }
        } else {
            if rendered_track.take().is_some() {
                rendered_status = None;
                resume.hide();
                find.hide();
                title_update.set_text("Текст песни");
                artist_update.set_text("Выбери трек");
                rendered_labels = None;
                label_update.set_text("Выбери трек");
                hint_update.set_text("");
                body_update.set_visible_child_name("empty");
            }
            if rendered_cover.take().is_some() {
                backdrop_source_update.replace(None);
                backdrop_previous.replace(None);
                backdrop_mix.snap(1.0);
                backdrop_update.queue_draw();
            }
        }
    });
    overlay.upcast()
}

fn update_lyric_focus(
    list: &gtk::ListBox,
    previous: Option<(Option<usize>, bool)>,
    (active, browsing): (Option<usize>, bool),
    count: usize,
) {
    let (old_active, old_browsing) = previous.unwrap_or((None, false));
    let past_end = |index: Option<usize>, browsing| if browsing { 0 } else { index.unwrap_or(0) };
    let old_end = past_end(old_active, old_browsing);
    let new_end = past_end(active, browsing);
    for index in old_end.min(new_end)..old_end.max(new_end).min(count) {
        if let Some(row) = list.row_at_index(index as i32) {
            set_style_class(&row, "past", index < new_end);
        }
    }
    for index in [old_active, active].into_iter().flatten() {
        if let Some(row) = list.row_at_index(index as i32) {
            set_style_class(&row, "active", active == Some(index));
        }
    }
}

thread_local! {
    static DECORATIVE_MOTION: Cell<bool> = const { Cell::new(true) };
}

fn animations_enabled() -> bool {
    DECORATIVE_MOTION.with(Cell::get)
        && gtk::Settings::default()
            .is_none_or(|settings| settings.property::<bool>("gtk-enable-animations"))
}

fn set_decorative_motion(enabled: bool) {
    DECORATIVE_MOTION.with(|setting| setting.set(enabled));
    theme::set_motion(enabled);
}

fn sync_stack_motion(widget: &gtk::Widget) {
    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        stack.set_transition_duration(if DECORATIVE_MOTION.with(Cell::get) {
            180
        } else {
            0
        });
    }
    if let Some(container) = widget.downcast_ref::<gtk::Container>() {
        for child in container.children() {
            sync_stack_motion(&child);
        }
    }
}

#[derive(Clone)]
struct QueueRow {
    row: gtk::ListBoxRow,
    path: Rc<RefCell<PathBuf>>,
    number: gtk::Label,
    cover: gtk::Image,
    cover_stack: gtk::Stack,
    rendered_cover: RefCell<Option<PathBuf>>,
    title: gtk::Label,
    artist: gtk::Label,
    duration: gtk::Label,
}

const QUEUE_PAGE_SIZE: usize = 120;

fn make_queue_row(
    position: usize,
    model: &Rc<RefCell<Model>>,
    menu: &gtk::Menu,
    menu_target: &Rc<RefCell<PathBuf>>,
) -> QueueRow {
    let row = gtk::ListBoxRow::new();
    row.style_context().add_class("queue-row");
    row.set_no_show_all(true);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_margin_top(6);
    line.set_margin_bottom(6);
    line.set_margin_start(12);
    line.set_margin_end(12);
    let number = gtk::Label::new(Some(&(position + 1).to_string()));
    number.style_context().add_class("queue-index");
    line.pack_start(&number, false, false, 0);
    let cover_stack = gtk::Stack::new();
    cover_stack.style_context().add_class("queue-cover");
    cover_stack.add_named(&cover_placeholder(44, 20), "placeholder");
    let cover = gtk::Image::new();
    cover.set_size_request(44, 44);
    cover_stack.add_named(&cover, "image");
    cover_stack.set_visible_child_name("placeholder");
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(None);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_max_width_chars(48);
    title.style_context().add_class("queue-title");
    let artist = gtk::Label::new(None);
    artist.set_xalign(0.0);
    artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    artist.set_max_width_chars(48);
    artist.style_context().add_class("queue-artist");
    titles.pack_start(&title, false, false, 0);
    titles.pack_start(&artist, false, false, 0);
    let duration = gtk::Label::new(None);
    duration.style_context().add_class("queue-duration");
    line.pack_start(&cover_stack, false, false, 0);
    line.pack_start(&titles, true, true, 0);
    line.pack_start(&duration, false, false, 0);
    let path = Rc::new(RefCell::new(PathBuf::new()));
    let actions = icon_button("view-more-symbolic", "Действия с треком", "queue-menu");
    actions.set_focus_on_click(true);
    let context_menu = menu.downgrade();
    let context_target = menu_target.clone();
    let context_path = path.clone();
    row.connect_button_press_event(move |row, event| {
        if event.triggers_context_menu() {
            if let Some(menu) = context_menu.upgrade() {
                context_target.replace(context_path.borrow().clone());
                menu.set_attach_widget(Some(row));
                menu.popup_at_pointer(Some(event));
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    let menu = menu.downgrade();
    let menu_target = menu_target.clone();
    let menu_path = path.clone();
    actions.connect_clicked(move |button| {
        if let Some(menu) = menu.upgrade() {
            menu_target.replace(menu_path.borrow().clone());
            menu.set_attach_widget(Some(button));
            if let Some(event) = gtk::current_event() {
                menu.popup_at_widget(
                    button,
                    gtk::gdk::Gravity::SouthEast,
                    gtk::gdk::Gravity::NorthEast,
                    Some(&event),
                );
            } else {
                menu.popup_easy(0, gtk::current_event_time());
            }
        }
    });
    line.pack_start(&actions, false, false, 0);
    row.add(&line);

    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let targets = [gtk::TargetEntry::new(
        "application/x-own-player-track",
        gtk::TargetFlags::SAME_APP,
        0,
    )];
    let handle = gtk::EventBox::new();
    handle.set_tooltip_text(Some("Перетащи трек, чтобы изменить порядок"));
    handle.add(&symbolize("list-drag-handle-symbolic", 16));
    handle.style_context().add_class("queue-handle");
    line.pack_start(&handle, false, false, 0);
    handle.drag_source_set(
        gtk::gdk::ModifierType::BUTTON1_MASK,
        &targets,
        gtk::gdk::DragAction::MOVE,
    );
    let drag_snapshot = Rc::new(RefCell::new(PathBuf::new()));
    let drag_start = drag_snapshot.clone();
    let source = path.clone();
    handle.connect_drag_begin(move |_, _| {
        drag_start.replace(source.borrow().clone());
    });
    handle.connect_drag_data_get(move |_, _, data, _, _| {
        data.set(
            &data.target(),
            8,
            drag_snapshot.borrow().as_os_str().as_bytes(),
        );
    });
    row.drag_dest_set(gtk::DestDefaults::ALL, &targets, gtk::gdk::DragAction::MOVE);
    let target = path.clone();
    let model = model.clone();
    row.connect_drag_data_received(move |row, context, _, y, data, _, time| {
        let source = PathBuf::from(std::ffi::OsString::from_vec(data.data()));
        let target = target.borrow().clone();
        let after = y >= row.allocated_height() / 2;
        let moved = edit_queue(&model, &source, QueueEdit::Drop(&target, after));
        context.drag_finish(moved, false, time);
    });

    line.show_all();
    QueueRow {
        row,
        path,
        number,
        cover,
        cover_stack,
        rendered_cover: RefCell::new(None),
        title,
        artist,
        duration,
    }
}

impl QueueRow {
    fn update(&mut self, track: &Track, index: usize, current: bool, next: bool) {
        if *self.path.borrow() != track.path {
            self.path.replace(track.path.clone());
            if self.rendered_cover.borrow().as_ref() != track.cover_path.as_ref() {
                self.clear_cover();
            }
        }
        set_style_class(&self.row, "queued-next", next);
        self.highlight(index, current);
        let mut subtitle = if track.album.is_empty() {
            track.artist.clone()
        } else {
            format!("{}  ·  {}", track.artist, track.album)
        };
        if next {
            subtitle.push_str("  ·  Следующий");
        }
        let duration = if track.duration > 0.0 {
            format_time(track.duration)
        } else {
            String::new()
        };
        if self.title.text() != track.title {
            self.title.set_text(&track.title);
            self.title.set_tooltip_text(Some(&track.title));
        }
        if self.artist.text() != subtitle {
            self.artist.set_text(&subtitle);
            self.artist.set_tooltip_text(Some(&subtitle));
        }
        if self.duration.text() != duration {
            self.duration.set_text(&duration);
        }
        self.row.show();
    }

    fn highlight(&self, index: usize, current: bool) {
        let number = if current {
            "♪".into()
        } else {
            (index + 1).to_string()
        };
        if self.number.text() != number {
            self.number.set_text(&number);
        }
        let tooltip = if current {
            Some("Текущий трек")
        } else {
            self.row
                .style_context()
                .has_class("queued-next")
                .then_some("Следующий трек")
        };
        if self.number.tooltip_text().as_deref() != tooltip {
            self.number.set_tooltip_text(tooltip);
        }
        set_style_class(&self.row, "playing", current);
    }

    fn clear_cover(&self) {
        self.rendered_cover.replace(None);
        self.cover_stack.set_visible_child_name("placeholder");
        self.cover.clear();
    }
}

fn queue_search_key(track: &Track) -> String {
    format!("{} {} {}", track.title, track.artist, track.album).to_lowercase()
}

fn set_style_class(widget: &impl IsA<gtk::Widget>, class: &str, enabled: bool) {
    if enabled {
        widget.style_context().add_class(class);
    } else {
        widget.style_context().remove_class(class);
    }
}

#[derive(Clone, Copy)]
enum QueueEdit<'a> {
    Play,
    PlayNext,
    Up,
    Down,
    Remove,
    Drop(&'a Path, bool),
}

fn menu_button(menu: &gtk::Menu, tooltip: &str) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_image(Some(&symbolize("view-more-symbolic", 18)));
    button.set_tooltip_text(Some(tooltip));
    button.style_context().add_class("queue-menu");
    if let Some(accessible) = button.accessible() {
        accessible.set_name(tooltip);
    }
    menu.show_all();
    button.set_popup(Some(menu));
    motion::animate_button(&button);
    button
}

fn edit_queue(model: &Rc<RefCell<Model>>, path: &Path, action: QueueEdit<'_>) -> bool {
    let mut state = model.borrow_mut();
    state.updates.request();
    let Some(index) = state.tracks.iter().position(|track| track.path == path) else {
        return false;
    };
    let current = state.current.map(|index| state.tracks[index].path.clone());
    let destination = match action {
        QueueEdit::Play => {
            drop(state);
            play_index(model, index);
            return true;
        }

        QueueEdit::PlayNext => {
            if current.as_deref() == Some(path) {
                return false;
            }
            if state.play_next.as_deref() != Some(path) {
                let previous = state.play_next.replace(path.to_path_buf());
                if let Some(previous) = previous {
                    if let Some(index) =
                        state.tracks.iter().position(|track| track.path == previous)
                    {
                        state.queue_metadata_changed.insert(index);
                    }
                }
                state.queue_metadata_changed.insert(index);
            }
            drop(state);
            save_session(model);
            return true;
        }
        QueueEdit::Up => index.saturating_sub(1),
        QueueEdit::Down => (index + 1).min(state.tracks.len() - 1),
        QueueEdit::Drop(target, after) => {
            let Some(target) = state.tracks.iter().position(|track| track.path == target) else {
                return false;
            };
            if target == index {
                return false;
            }
            let boundary = target + usize::from(after);
            boundary - usize::from(index < boundary)
        }
        QueueEdit::Remove => index,
    };
    if matches!(action, QueueEdit::Remove) {
        state.tracks.remove(index);
        state.shuffle_order.remove(path);
        if state.play_next.as_deref() == Some(path) {
            state.play_next = None;
        }
    } else if destination != index {
        let track = state.tracks.remove(index);
        state.tracks.insert(destination, track);
    } else {
        return false;
    }
    state.current =
        current.and_then(|path| state.tracks.iter().position(|track| track.path == path));
    let removed_current = state.current.is_none() && matches!(action, QueueEdit::Remove);
    let continue_at = (removed_current
        && state.mpv.as_ref().is_some_and(|p| p.loaded && !p.paused))
    .then(|| index.min(state.tracks.len().saturating_sub(1)))
    .filter(|_| !state.tracks.is_empty());
    state.library_revision += 1;
    state.failed_tracks.clear();
    drop(state);
    if let Some(index) = continue_at {
        play_index(model, index);
    } else if removed_current {
        stop_playback(model);
    }
    save_session(model);
    true
}

fn clear_queue(model: &Rc<RefCell<Model>>) {
    stop_playback(model);
    let mut state = model.borrow_mut();
    state.tracks.clear();
    state.current = None;
    state.play_next = None;
    state.shuffle_order = ShuffleOrder::default();
    state.failed_tracks.clear();
    state.library_revision += 1;
    drop(state);
    save_session(model);
}

fn build_queue_page(model: &Rc<RefCell<Model>>) -> (gtk::Widget, gtk::SearchEntry) {
    let row_menu = gtk::Menu::new();
    let menu_target = Rc::new(RefCell::new(PathBuf::new()));
    for (label, action) in [
        ("Воспроизвести", QueueEdit::Play),
        ("Поставить следующим", QueueEdit::PlayNext),
        ("Переместить выше", QueueEdit::Up),
        ("Переместить ниже", QueueEdit::Down),
        ("Убрать из очереди", QueueEdit::Remove),
    ] {
        let item = gtk::MenuItem::with_label(label);
        if matches!(action, QueueEdit::PlayNext) {
            let item = item.downgrade();
            let model = model.clone();
            let target = menu_target.clone();
            row_menu.connect_map(move |_| {
                let state = model.borrow();
                let current = state.current.and_then(|index| state.tracks.get(index));
                if let Some(item) = item.upgrade() {
                    item.set_visible(current.is_none_or(|track| track.path != *target.borrow()));
                }
            });
        }
        let model = model.clone();
        let path = menu_target.clone();
        item.connect_activate(move |_| {
            edit_queue(&model, &path.borrow(), action);
        });
        row_menu.append(&item);
    }
    row_menu.append(&gtk::SeparatorMenuItem::new());
    track_menu::add_details(&row_menu, model, &menu_target);
    row_menu.show_all();

    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let summary = gtk::Label::new(None);
    summary.set_xalign(0.0);
    summary.style_context().add_class("queue-summary");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let heading = gtk::Label::new(Some("Очередь"));
    heading.style_context().add_class("heading");
    header.pack_start(&heading, false, false, 0);
    header.pack_end(&summary, false, false, 0);
    page.pack_start(&header, false, false, 0);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.style_context().add_class("queue-toolbar");
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Найти в очереди"));
    describe_control(&search, "Поиск по названию, исполнителю и альбому");
    search.connect_stop_search(|entry| entry.set_text(""));
    search.style_context().add_class("search");
    toolbar.pack_start(&search, true, true, 0);
    let menu = gtk::Menu::new();
    let start = gtk::MenuItem::with_label("Воспроизвести с начала");
    let model_start = model.clone();
    start.connect_activate(move |_| play_index(&model_start, 0));
    menu.append(&start);
    let clear_all = gtk::MenuItem::with_label("Очистить очередь…");
    let model_clear = model.clone();
    clear_all.connect_activate(move |_| {
        let parent = active_window();
        let dialog = gtk::MessageDialog::new(
            parent.as_ref(),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::None,
            "Очистить очередь?",
        );
        dialog.set_secondary_text(Some("Аудиофайлы останутся на диске."));
        dialog.add_button("Отмена", gtk::ResponseType::Cancel);
        dialog.add_button("Очистить", gtk::ResponseType::Accept);
        if dialog.run() == gtk::ResponseType::Accept {
            clear_queue(&model_clear);
        }
        dialog.close();
    });
    menu.append(&clear_all);
    let queue_actions = menu_button(&menu, "Действия с очередью");
    queue_actions.style_context().add_class("queue-add");
    let add = icon_button(
        "list-add-symbolic",
        "Добавить музыку в очередь",
        "queue-add",
    );
    let model_add = model.clone();
    add.connect_clicked(move |_| {
        if let Some(paths) = choose_files(false) {
            append_paths(&model_add, paths);
        }
    });
    toolbar.pack_start(&add, false, false, 0);
    toolbar.pack_start(&queue_actions, false, false, 0);
    page.pack_start(&toolbar, false, false, 0);
    let body = gtk::Stack::new();
    body.set_transition_type(gtk::StackTransitionType::Crossfade);
    body.set_transition_duration(200);
    let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    empty_box.set_halign(gtk::Align::Center);
    empty_box.set_valign(gtk::Align::Center);
    let empty_icon = symbolize("view-list-symbolic", 40);
    empty_icon.style_context().add_class("empty-glyph");
    empty_box.pack_start(&empty_icon, false, false, 0);
    let empty_title = gtk::Label::new(Some("Очередь пуста"));
    empty_title.style_context().add_class("empty-title");
    empty_box.pack_start(&empty_title, false, false, 0);
    let empty_note = gtk::Label::new(Some("Добавь файлы или папку с музыкой"));
    empty_note.style_context().add_class("secondary");
    empty_box.pack_start(&empty_note, false, false, 0);
    let empty_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    empty_panel.style_context().add_class("queue-panel");
    empty_panel.pack_start(&empty_box, true, true, 0);
    body.add_named(&empty_panel, "empty");
    let no_results = gtk::Box::new(gtk::Orientation::Vertical, 12);
    no_results.set_halign(gtk::Align::Center);
    no_results.set_valign(gtk::Align::Center);
    let no_icon = symbolize("system-search-symbolic", 32);
    no_icon.style_context().add_class("empty-glyph");
    no_results.pack_start(&no_icon, false, false, 0);
    let no_title = gtk::Label::new(Some("Ничего не нашлось"));
    no_title.style_context().add_class("empty-title");
    let no_note = gtk::Label::new(Some("Попробуй другое название, исполнителя или альбом"));
    no_note.set_justify(gtk::Justification::Center);
    no_note.set_line_wrap(true);
    no_note.set_max_width_chars(36);
    no_note.style_context().add_class("secondary");
    let clear = gtk::Button::with_label("Сбросить поиск");
    clear.set_halign(gtk::Align::Center);
    let search_clear = search.clone();
    clear.connect_clicked(move |_| search_clear.set_text(""));
    no_results.pack_start(&no_title, false, false, 0);
    no_results.pack_start(&no_note, false, false, 0);
    no_results.pack_start(&clear, false, false, 8);
    let results_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    results_panel.style_context().add_class("queue-panel");
    results_panel.pack_start(&no_results, true, true, 0);
    body.add_named(&results_panel, "no-results");

    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.style_context().add_class("queue-card");
    scroll.set_overlay_scrolling(true);
    scroll.set_shadow_type(gtk::ShadowType::None);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    list.style_context().add_class("queue-list");
    scroll.add(&list);
    let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
    table.style_context().add_class("queue-table");
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    columns.style_context().add_class("queue-columns");
    let number = gtk::Label::new(Some("#"));
    number.style_context().add_class("queue-index");
    let name = gtk::Label::new(Some("Трек"));
    name.set_xalign(0.0);
    name.set_margin_start(56);
    let duration = gtk::Label::new(Some("Время"));
    duration.style_context().add_class("queue-duration");
    duration.set_margin_end(96);
    columns.pack_start(&number, false, false, 0);
    columns.pack_start(&name, true, true, 0);
    columns.pack_start(&duration, false, false, 0);
    table.pack_start(&columns, false, false, 0);
    table.pack_start(&scroll, true, true, 0);
    body.add_named(&table, "list");
    body.set_visible_child_name("empty");
    page.pack_start(&body, true, true, 0);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    footer.style_context().add_class("queue-pagination");
    footer.set_halign(gtk::Align::End);
    let previous = icon_button("go-previous-symbolic", "Предыдущая страница", "page-arrow");
    let next = icon_button("go-next-symbolic", "Следующая страница", "page-arrow");
    let page_number = gtk::Entry::new();
    page_number.set_width_chars(4);
    page_number.set_max_width_chars(5);
    page_number.set_alignment(0.5);
    page_number.set_input_purpose(gtk::InputPurpose::Digits);
    page_number.set_text("1");
    describe_control(&page_number, "Страница очереди");
    let page_count = gtk::Label::new(None);
    page_count.style_context().add_class("queue-page-count");
    footer.pack_start(&previous, false, false, 0);
    footer.pack_start(&page_number, false, false, 0);
    footer.pack_start(&page_count, false, false, 4);
    footer.pack_start(&next, false, false, 0);
    let updates = model.borrow().updates.clone();
    let requested_page = Rc::new(Cell::new(1_usize));
    let last_page = Rc::new(Cell::new(1_usize));
    previous.connect_clicked({
        let requested = requested_page.clone();
        let updates = updates.clone();
        move |_| {
            requested.set(requested.get().saturating_sub(1).max(1));
            updates.request();
        }
    });
    next.connect_clicked({
        let requested = requested_page.clone();
        let last_page = last_page.clone();
        let updates = updates.clone();
        move |_| {
            requested.set((requested.get() + 1).min(last_page.get()));
            updates.request();
        }
    });
    page_number.connect_activate({
        let requested = requested_page.clone();
        let last_page = last_page.clone();
        let updates = updates.clone();
        move |entry| {
            if let Ok(value) = entry.text().trim().parse::<usize>() {
                requested.set(value.clamp(1, last_page.get()));
            }
            entry.set_text(&requested.get().to_string());
            updates.request();
        }
    });
    footer.show_all();
    footer.set_no_show_all(true);
    footer.hide();
    page.pack_start(&footer, false, false, 0);

    let jump = gtk::MenuItem::with_label("Показать текущий трек");
    menu.prepend(&jump);
    jump.show();
    let jump_requested = Rc::new(Cell::new(false));
    let request = jump_requested.clone();
    let jump_search = search.clone();
    jump.connect_activate({
        let updates = updates.clone();
        move |_| {
            jump_search.set_text("");
            request.set(true);
            updates.request();
        }
    });

    let rows: Rc<RefCell<Vec<QueueRow>>> = Rc::new(RefCell::new(Vec::new()));

    let model_play = model.clone();
    let rows_play = rows.clone();
    list.connect_row_activated(move |_, row| {
        let index = rows_play
            .borrow()
            .get(row.index() as usize)
            .and_then(|item| {
                let path = item.path.borrow();
                model_play
                    .borrow()
                    .tracks
                    .iter()
                    .position(|track| track.path == *path)
            });
        if let Some(index) = index {
            play_index(&model_play, index);
        }
    });

    let query = Rc::new(RefCell::new(String::new()));
    let search_query = query.clone();
    search.connect_search_changed({
        let updates = updates.clone();
        move |entry| {
            search_query.replace(entry.text().trim().to_lowercase());
            updates.request();
        }
    });
    let adjustment = scroll.vadjustment();
    adjustment.connect_value_changed({
        let updates = updates.clone();
        move |_| updates.request()
    });
    adjustment.connect_changed({
        let updates = updates.clone();
        move |_| updates.request()
    });

    let model_update = model.clone();
    let rows_update = rows.clone();
    let page_update = page.downgrade();
    let mut rendered_revision = u64::MAX;
    let mut rendered_query = String::new();
    let mut search_keys = Vec::new();
    let mut matches = Vec::new();
    let mut page_indices = Vec::new();
    let mut render_index = 0;
    let mut rendered_current = None;
    let mut selected_page = usize::MAX;
    let mut jump_to = None;
    let mut visible_covers = Vec::new();
    updates.listen(&page, move || {
        if !page_update.upgrade().is_some_and(|page| page.is_mapped()) {
            return;
        }
        let mut state = model_update.borrow_mut();
        let track_count = state.tracks.len();
        let order_changed = rendered_revision != state.library_revision;
        rendered_revision = state.library_revision;
        let changed = std::mem::take(&mut state.queue_metadata_changed);
        let query = query.borrow();
        let query_changed = rendered_query != *query;
        if order_changed {
            search_keys.clear();
        }
        for index in changed.iter().copied() {
            if let Some((key, track)) = search_keys.get_mut(index).zip(state.tracks.get(index)) {
                *key = queue_search_key(track);
            }
        }
        if order_changed || query_changed || (!query.is_empty() && !changed.is_empty()) {
            matches.clear();
            if query.is_empty() {
                matches.extend(0..track_count);
            } else {
                if search_keys.is_empty() {
                    search_keys.extend(state.tracks.iter().map(queue_search_key));
                }
                matches.extend(
                    search_keys
                        .iter()
                        .enumerate()
                        .filter_map(|(index, key)| key.contains(query.as_str()).then_some(index)),
                );
            }
            rendered_query.clone_from(&query);
        }

        let pages = matches.len().div_ceil(QUEUE_PAGE_SIZE).max(1);
        if last_page.replace(pages) != pages {
            page_count.set_text(&format!("/ {pages}"));
        }
        if query_changed {
            requested_page.set(1);
        }
        if jump_requested.get() && query.is_empty() {
            jump_requested.set(false);
            jump_to = state.current;
            if let Some(index) = state.current {
                requested_page.set(index / QUEUE_PAGE_SIZE + 1);
            }
        }
        let current_page = requested_page.get().min(pages);
        requested_page.set(current_page);
        let page_index = current_page - 1;
        if footer.is_visible() != (pages > 1) {
            footer.set_visible(pages > 1);
        }
        previous.set_sensitive(page_index > 0);
        next.set_sensitive(page_index + 1 < pages);
        jump.set_sensitive(state.current.is_some());
        let start_index = page_index * QUEUE_PAGE_SIZE;
        let indices = &matches[start_index..(start_index + QUEUE_PAGE_SIZE).min(matches.len())];
        let new_page = selected_page != page_index || query_changed;
        if new_page {
            selected_page = page_index;
            page_number.set_text(&current_page.to_string());
            if !query.is_empty() {
                jump_to = None;
            }
            row_menu.popdown();
            list.unselect_all();
            scroll.vadjustment().set_value(0.0);
        }
        if order_changed || page_indices != indices || new_page {
            page_indices.clear();
            page_indices.extend_from_slice(indices);
            render_index = 0;
        }
        let mut rows = rows_update.borrow_mut();
        while rows.len() > page_indices.len() {
            if let Some(item) = rows.pop() {
                list.remove(&item.row);
            }
        }
        let deadline = Instant::now() + Duration::from_millis(6);
        while render_index < page_indices.len() && Instant::now() < deadline {
            let index = page_indices[render_index];
            if render_index == rows.len() {
                let item = make_queue_row(index, &model_update, &row_menu, &menu_target);
                list.add(&item.row);
                rows.push(item);
            }
            let track = &state.tracks[index];
            rows[render_index].update(
                track,
                index,
                state.current == Some(index),
                state.play_next.as_ref() == Some(&track.path),
            );
            render_index += 1;
        }
        for index in changed.iter().copied() {
            if let Ok(slot) = page_indices.binary_search(&index) {
                if slot < render_index {
                    let track = &state.tracks[index];
                    rows[slot].update(
                        track,
                        index,
                        state.current == Some(index),
                        state.play_next.as_ref() == Some(&track.path),
                    );
                }
            }
        }
        if state.current != rendered_current {
            for index in [rendered_current, state.current].into_iter().flatten() {
                if let Ok(slot) = page_indices.binary_search(&index) {
                    if slot < render_index {
                        rows[slot].highlight(index, state.current == Some(index));
                    }
                }
            }
            rendered_current = state.current;
        }
        let complete = render_index == page_indices.len();
        if let Some(index) = jump_to {
            if let Ok(slot) = page_indices.binary_search(&index) {
                if let Some(item) = rows
                    .get(slot)
                    .filter(|item| item.row.allocated_height() > 1 && slot < render_index)
                {
                    let bounds = item.row.allocation();
                    let adjustment = scroll.vadjustment();
                    adjustment.set_value(f64::from(bounds.y()) - adjustment.page_size() / 2.0);
                    list.select_row(Some(&item.row));
                    item.row.grab_focus();
                    jump_to = None;
                }
            } else {
                jump_to = None;
            }
        }
        drop(rows);
        let visible = if track_count == 0 {
            "empty"
        } else if matches.is_empty() {
            "no-results"
        } else {
            "list"
        };
        if body.visible_child_name().as_deref() != Some(visible) {
            body.set_visible_child_name(visible);
        }
        if order_changed || query_changed || !changed.is_empty() {
            let total: f64 = matches
                .iter()
                .map(|&index| state.tracks[index].duration)
                .sum();
            let hours = (total / 3600.0) as u64;
            let minutes = (total / 60.0) as u64 % 60;
            let duration = if hours > 0 {
                format!(" · {hours} ч {minutes} мин")
            } else if minutes > 0 {
                format!(" · {minutes} мин")
            } else {
                String::new()
            };
            let text = if query.is_empty() {
                format!("{}{duration}", tracks_phrase(track_count))
            } else {
                format!("{} из {track_count}{duration}", matches.len())
            };
            if summary.text() != text {
                summary.set_text(&text);
            }
        }
        let can_start = track_count > 0 && state.mpv.is_some();
        if start.is_sensitive() != can_start {
            start.set_sensitive(can_start);
        }
        if clear_all.is_sensitive() != (track_count > 0) {
            clear_all.set_sensitive(track_count > 0);
        }
        if !complete || jump_to.is_some() {
            state.updates.request();
        }
        drop(state);
        // Load only visible rows (+ a small buffer), with bounded background work.
        if scroll.is_mapped() {
            let state = model_update.borrow();
            let adjustment = scroll.vadjustment();
            let top = adjustment.value();
            let bottom = top + adjustment.page_size();
            let available = 4usize.saturating_sub(state.cover_pending.len());
            let mut jobs = Vec::new();
            let first = list
                .row_at_y((top - 160.0).max(4.0) as i32)
                .map_or(0, |row| row.index() as usize);
            let rows = rows_update.borrow();
            let mut visible = Vec::new();
            for (index, (item, &track_index)) in rows
                .iter()
                .zip(&page_indices)
                .enumerate()
                .take(render_index)
                .skip(first)
            {
                let track = &state.tracks[track_index];
                let bounds = item.row.allocation();
                let y = f64::from(bounds.y());
                let end = y + f64::from(bounds.height());
                if y > bottom + 160.0 {
                    break;
                }
                if end < top - 160.0 {
                    continue;
                }
                visible.push(index);
                if *item.rendered_cover.borrow() != track.cover_path {
                    if let Some(image) =
                        artwork::get(track.cover_path.as_deref(), false, &state.updates)
                    {
                        if let Some(image) = image {
                            item.cover.set_from_pixbuf(Some(&image.thumb));
                            item.cover_stack.set_visible_child_name("image");
                        } else {
                            item.cover.clear();
                            item.cover_stack.set_visible_child_name("placeholder");
                        }
                        *item.rendered_cover.borrow_mut() = track.cover_path.clone();
                    }
                }
                if available > 0
                    && track.cover_path.is_none()
                    && state.metadata_ready.contains(&track.path)
                    && !state.cover_pending.contains(&track.path)
                    && !state.cover_attempted.contains(&track.path)
                {
                    jobs.push(((top - end).max(y - bottom).max(0.0), &track.path));
                }
            }
            if visible != visible_covers {
                for index in visible_covers.drain(..) {
                    if let Some(item) = rows
                        .get(index)
                        .filter(|_| visible.binary_search(&index).is_err())
                    {
                        item.clear_cover();
                    }
                }
                visible_covers = visible;
            }
            drop(rows);
            jobs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let paths: Vec<_> = jobs
                .into_iter()
                .take(available)
                .map(|(_, path)| path.clone())
                .collect();
            drop(state);
            for path in paths {
                start_cover_job(&model_update, path);
            }
        }
    });
    (page.upcast(), search)
}

fn start_mpv(model: &Rc<RefCell<Model>>) {
    request_ui(model);
    let Some((child, path)) = Mpv::spawn() else {
        model.borrow_mut().status =
            Some("Не удалось запустить mpv. Проверь, что он установлен.".into());
        return;
    };
    let task = gtk::gio::spawn_blocking(move || Mpv::connect(child, path));
    let model = Rc::downgrade(model);
    glib::MainContext::default().spawn_local(async move {
        let connected = task.await.ok().flatten();
        let Some(model) = model.upgrade() else { return };
        match connected {
            Some(mut player) => {
                let mut state = model.borrow_mut();
                let _ = player.set_volume(state.volume);
                let _ = player.set_muted(state.muted);
                let _ = player.command(json!(["set_property", "speed", state.speed]));
                let _ = player.command(json!([
                    "set_property",
                    "loop-file",
                    if state.repeat == 2 { "inf" } else { "no" }
                ]));
                let events_model = Rc::downgrade(&model);
                player.event_source = Some(glib::unix_fd_add_local(
                    player.socket_fd(),
                    glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
                    move |_, _| {
                        if let Some(model) = events_model.upgrade() {
                            tick_mpv(&model);
                        }
                        glib::ControlFlow::Continue
                    },
                ));
                state.mpv = Some(player);
                state.updates.request();
            }
            None => {
                let mut state = model.borrow_mut();
                state.status = Some("Не удалось запустить mpv. Проверь, что он установлен.".into());
                state.updates.request();
            }
        }
    });
}

fn arm_mpv_writes(model: &Rc<RefCell<Model>>) {
    let mut state = model.borrow_mut();
    let Some(player) = state
        .mpv
        .as_mut()
        .filter(|p| p.has_outgoing() && p.write_source.is_none())
    else {
        return;
    };
    let model = Rc::downgrade(model);
    player.write_source = Some(glib::unix_fd_add_local(
        player.socket_fd(),
        glib::IOCondition::OUT,
        move |_, _| {
            let Some(model) = model.upgrade() else {
                return glib::ControlFlow::Break;
            };
            tick_mpv(&model);
            if let Some(player) = model.borrow_mut().mpv.as_mut() {
                if player.has_outgoing() {
                    return glib::ControlFlow::Continue;
                }
                player.write_source.take();
            }
            glib::ControlFlow::Break
        },
    ));
}

fn tick_mpv(model: &Rc<RefCell<Model>>) {
    let (ended, failed) = {
        let mut state = model.borrow_mut();
        state.updates.request_progress();
        let Some(player) = state.mpv.as_mut() else {
            return;
        };
        if let Err(error) = player.read_events() {
            state.mpv = None;
            state.status = Some(format!(
                "Связь с mpv прервалась: {error}. Перезапусти плеер."
            ));
            return;
        }
        let volume = player.volume.take();
        let muted = player.muted.take();
        let ended = std::mem::take(&mut player.ended);
        let failed = std::mem::take(&mut player.failed);
        if let Some(error) = player.error.take() {
            state.status = Some(error);
        }
        if let Some(volume) = volume {
            state.volume = volume.clamp(0.0, 100.0);
        }
        if let Some(muted) = muted {
            state.muted = muted;
        }
        (ended, failed)
    };
    if failed {
        let mut state = model.borrow_mut();
        if let Some(current) = state.current {
            state.failed_tracks.insert(current);
            let next = advance_index(&mut state, 1, true);
            drop(state);
            if let Some(next) = next {
                load_index(model, next);
                let mut state = model.borrow_mut();
                if state.status.is_none() {
                    state.status = Some(
                        "Не удалось воспроизвести предыдущий файл. Перешли к следующему треку."
                            .into(),
                    );
                }
            }
        }
    } else if ended {
        model.borrow_mut().failed_tracks.clear();
        next_track(model, 1, true);
    }
}

fn play_index(model: &Rc<RefCell<Model>>, index: usize) {
    model.borrow_mut().failed_tracks.clear();
    load_index(model, index);
}

fn load_index(model: &Rc<RefCell<Model>>, index: usize) {
    let mut state = model.borrow_mut();
    state.updates.request();
    if index >= state.tracks.len() {
        return;
    }
    if state.mpv.is_none() {
        state.status = Some(
            "Не удалось запустить mpv. Проверь, что он установлен, и перезапусти плеер.".into(),
        );
        return;
    }
    let path = state.tracks[index].path.clone();
    let Some(mpv_path) = path.to_str() else {
        state.status = Some("Имя файла содержит символы, которые mpv IPC не принимает.".into());
        return;
    };
    state.status = None;
    if let Some(player) = state.mpv.as_mut() {
        if player
            .command(json!(["loadfile", mpv_path, "replace"]))
            .is_ok()
        {
            player.generation = player.generation.wrapping_add(1);
            player.set_position(0.0);
            player.duration = 0.0;
            player.ended = false;
            player.paused = false;
            player.loaded = true;
        } else {
            state.status = Some("Не удалось передать трек в mpv. Перезапусти плеер.".into());
            return;
        }
        let _ = player.command(json!(["set_property", "pause", false]));
    }
    state.current = Some(index);
    if state.shuffle {
        state.shuffle_order.record(&path);
    }
    if state.play_next.as_ref() == Some(&path) {
        state.play_next = None;
        state.queue_metadata_changed.insert(index);
    }
    drop(state);
    save_session(model);
    if !model.borrow().metadata_ready.contains(&path) && path.is_file() {
        start_metadata_job(model, vec![path.clone()]);
    }
    start_cover_job(model, path);
    start_lyrics_job(model, false);
}

fn next_track(model: &Rc<RefCell<Model>>, direction: isize, automatic: bool) {
    let mut state = model.borrow_mut();
    if state.tracks.is_empty() {
        return;
    }
    let index = advance_index(&mut state, direction, false);
    drop(state);
    if let Some(index) = index {
        play_index(model, index);
    } else if automatic {
        stop_playback(model);
    }
}

fn can_advance(state: &Model) -> bool {
    let Some(current) = state.current else {
        return true;
    };
    state.play_next.is_some()
        || state.repeat == 1
        || if state.shuffle {
            state.shuffle_order.can_advance(state.tracks.len())
        } else {
            current + 1 < state.tracks.len()
        }
}

fn advance_index(state: &mut Model, direction: isize, skip_failed: bool) -> Option<usize> {
    if direction > 0 {
        if let Some(path) = &state.play_next {
            if let Some(index) = state.tracks.iter().position(|track| &track.path == path) {
                return Some(index);
            }
        }
    }
    let Some(current) = state.current else {
        return (!state.tracks.is_empty()).then_some(0);
    };
    if state.shuffle {
        let tracks = state
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                (!skip_failed || !state.failed_tracks.contains(&index))
                    .then_some(track.path.as_path())
            });
        let current_path = &state.tracks[current].path;
        state.shuffle_order.record(current_path);
        let path = if direction < 0 {
            state.shuffle_order.previous()?.to_path_buf()
        } else {
            let mut path =
                state
                    .shuffle_order
                    .next(tracks.clone(), current_path, state.repeat == 1);
            while skip_failed
                && path
                    .as_ref()
                    .is_some_and(|path| !tracks.clone().any(|item| item == path))
            {
                path = state
                    .shuffle_order
                    .next(tracks.clone(), current_path, state.repeat == 1);
            }
            path?
        };
        return state.tracks.iter().position(|track| track.path == path);
    }
    if skip_failed {
        return (current + 1..state.tracks.len())
            .chain((0..current).filter(|_| state.repeat == 1))
            .find(|index| !state.failed_tracks.contains(index));
    }
    next_index(current, state.tracks.len(), direction, state.repeat)
}

fn next_index(current: usize, count: usize, direction: isize, repeat: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    if direction < 0 {
        return Some(if current == 0 && repeat == 1 {
            count - 1
        } else {
            current.saturating_sub(1)
        });
    }
    if current + 1 < count {
        Some(current + 1)
    } else if repeat == 1 {
        Some(0)
    } else {
        None
    }
}

fn stop_playback(model: &Rc<RefCell<Model>>) {
    let mut state = model.borrow_mut();
    state.updates.request();
    if let Some(player) = state.mpv.as_mut() {
        let _ = player.command(json!(["stop"]));
        player.paused = true;
        player.loaded = false;
        player.set_position(0.0);
        player.duration = 0.0;
    }
}

fn append_paths(model: &Rc<RefCell<Model>>, paths: Vec<PathBuf>) {
    {
        let mut state = model.borrow_mut();
        state.imports_pending += 1;
        state.updates.request();
        state.status = None;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut found = Vec::new();
        for path in paths {
            collect_audio(&path, &mut found);
        }
        found.sort();
        found.dedup();
        let total = found.len();
        // Session and mpv IPC use UTF-8 paths. Keep unsupported names out of saved queues.
        found.retain(|path| path.to_str().is_some());
        let unsupported = total - found.len();
        let _ = sender.send((found, unsupported));
    });
    let model_result = model.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok((paths, unsupported)) => {
                let mut state = model_result.borrow_mut();
                state.imports_pending = state.imports_pending.saturating_sub(1);
                state.updates.request();
                let found = !paths.is_empty();
                let skipped = || format!("Пропущено файлов с именами вне UTF-8: {unsupported}.");
                let mut fresh = Vec::new();
                let mut known: std::collections::HashSet<PathBuf> = state
                    .tracks
                    .iter()
                    .map(|track| track.path.clone())
                    .collect();
                for path in paths {
                    if known.insert(path.clone()) {
                        state.tracks.push(preview_track(path.clone()));
                        fresh.push(path);
                    }
                }
                if fresh.is_empty() {
                    state.status = Some(if unsupported > 0 {
                        skipped()
                    } else if found {
                        "Эти треки уже есть в очереди".into()
                    } else {
                        "Поддерживаемые аудиофайлы не найдены. Проверь папку и форматы файлов."
                            .into()
                    });
                    return glib::ControlFlow::Break;
                }
                state.status = None;
                state.library_revision += 1;
                let start = state.current.is_none();
                drop(state);
                save_session(&model_result);
                if start {
                    play_index(&model_result, 0);
                }
                if unsupported > 0 {
                    model_result.borrow_mut().status = Some(skipped());
                }
                start_metadata_job(&model_result, fresh);
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let mut state = model_result.borrow_mut();
                state.imports_pending = state.imports_pending.saturating_sub(1);
                state.updates.request();
                state.status = Some("Импорт прервался. Попробуй добавить файлы ещё раз.".into());
                glib::ControlFlow::Break
            }
        }
    });
}

fn restore_session(model: &Rc<RefCell<Model>>) {
    let saved = match session::load() {
        Ok(saved) => saved,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!("Не удалось восстановить очередь: {error}");
            return;
        }
    };
    let mut state = model.borrow_mut();
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for path in saved.paths {
        if seen.insert(path.clone()) {
            state.tracks.push(preview_track(path.clone()));
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    state.current = saved
        .current_path
        .and_then(|path| state.tracks.iter().position(|track| track.path == path));
    state.play_next = saved.play_next.filter(|path| {
        state.tracks.iter().any(|track| &track.path == path)
            && state
                .current
                .is_none_or(|index| &state.tracks[index].path != path)
    });
    state.shuffle = saved.shuffle;
    state.repeat = saved.repeat.min(2);
    state.online_covers = saved.online_covers;
    state.muted = saved.muted;
    state.appearance = saved.appearance;
    state.volume = saved.volume.clamp(0.0, 100.0);
    state.speed = saved.speed.clamp(0.5, 2.0);
    state.library_revision += 1;
    let current_path = state
        .current
        .and_then(|index| state.tracks.get(index))
        .map(|track| track.path.clone());
    drop(state);
    if let Some(path) = current_path.filter(|path| path.is_file()) {
        if let Some(position) = paths.iter().position(|item| item == &path) {
            paths.swap(0, position);
        }
        start_cover_job(model, path);
    }
    if !paths.is_empty() {
        start_metadata_job(model, paths);
    }
}

fn start_metadata_job(model: &Rc<RefCell<Model>>, mut paths: Vec<PathBuf>) {
    {
        let mut state = model.borrow_mut();
        paths.retain(|path| state.metadata_pending.insert(path.clone()));
    }
    if paths.is_empty() {
        return;
    }
    let indices: std::collections::HashMap<PathBuf, usize> = model
        .borrow()
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.path.clone(), index))
        .collect();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for path in paths {
            if sender.send(read_track(path)).is_err() {
                break;
            }
        }
    });
    let model_result = model.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let mut received = 0;
        while received < 32 {
            match receiver.try_recv() {
                Ok(track) => {
                    let mut state = model_result.borrow_mut();
                    let selected = state
                        .current
                        .and_then(|index| state.tracks.get(index))
                        .filter(|current| current.path == track.path)
                        .map(|current| current.path.clone());
                    merge_metadata(&mut state, &indices, track);
                    drop(state);
                    if let Some(path) = selected {
                        start_cover_job(&model_result, path);
                        start_lyrics_job(&model_result, false);
                    }
                    received += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn merge_metadata(
    state: &mut Model,
    indices: &std::collections::HashMap<PathBuf, usize>,
    mut track: Track,
) {
    state.metadata_pending.remove(&track.path);
    let index = indices
        .get(&track.path)
        .copied()
        .filter(|&index| {
            state
                .tracks
                .get(index)
                .is_some_and(|existing| existing.path == track.path)
        })
        .or_else(|| {
            state
                .tracks
                .iter()
                .position(|existing| existing.path == track.path)
        });
    if let Some(index) = index {
        let existing = &mut state.tracks[index];
        track.cover_path = existing.cover_path.take();
        if track.cover_path.is_none()
            && state.online_covers
            && (existing.title != track.title || existing.artist != track.artist)
        {
            state.cover_attempted.remove(&track.path);
        }
        if track.lyrics.is_empty() {
            track.lyrics = std::mem::take(&mut existing.lyrics);
        }
        state.metadata_ready.insert(track.path.clone());
        *existing = track;
        state.queue_metadata_changed.insert(index);
        state.updates.request();
    }
}

/// At most two requests at once; results belong to the file, never to the open view.
fn start_lyrics_job(model: &Rc<RefCell<Model>>, retry: bool) {
    let track = {
        let mut state = model.borrow_mut();
        let Some(path) = state
            .current
            .and_then(|i| state.tracks.get(i))
            .map(|track| track.path.clone())
        else {
            return;
        };
        if retry {
            state.lyrics_results.remove(&path);
        }
        let track = &state.tracks[state.current.unwrap()];
        if !state.metadata_ready.contains(&path)
            || !track.lyrics.is_empty()
            || state.lyrics_pending.contains(&path)
            || state.lyrics_results.contains_key(&path)
            || state.lyrics_pending.len() >= 2
        {
            return;
        }
        let track = track.clone();
        state.lyrics_pending.insert(path);
        state.updates.request();
        track
    };
    let path = track.path.clone();
    let task = gtk::gio::spawn_blocking(move || online_lyrics(&track));
    let model = Rc::downgrade(model);
    glib::MainContext::default().spawn_local(async move {
        let result = task
            .await
            .unwrap_or_else(|_| Err("Поиск прервался. Нажми «Искать снова».".into()));
        let Some(model) = model.upgrade() else { return };
        let mut state = model.borrow_mut();
        state.lyrics_pending.remove(&path);
        state.updates.request();
        let message = match result {
            Ok(lines) if !lines.is_empty() => {
                if let Some(track) = state.tracks.iter_mut().find(|track| track.path == path) {
                    if track.lyrics.is_empty() {
                        track.lyrics = lines;
                    }
                }
                "Текст найден".into()
            }
            Ok(_) => "Текст пока не найден. Можно попробовать ещё раз.".into(),
            Err(error) => error,
        };
        state.lyrics_results.insert(path.clone(), message);
        drop(state);
        start_lyrics_job(&model, false);
    });
}

fn start_cover_job(model: &Rc<RefCell<Model>>, path: PathBuf) {
    let (selected, online) = {
        let mut state = model.borrow_mut();
        let Some(selected) = state
            .tracks
            .iter()
            .find(|track| track.path == path)
            .cloned()
        else {
            return;
        };
        if selected.cover_path.is_some()
            || state.cover_attempted.contains(&path)
            || !state.cover_pending.insert(path.clone())
        {
            return;
        }
        state.cover_attempted.insert(path.clone());
        state.updates.request();
        (selected, state.online_covers)
    };
    let lookup_identity = (selected.title.clone(), selected.artist.clone());
    let task = gtk::gio::spawn_blocking(move || {
        extract_cover(&selected.path)
            .or_else(|| online.then(|| cover_art::online_cover(&selected)).flatten())
    });
    let model = Rc::downgrade(model);
    glib::MainContext::default().spawn_local(async move {
        let result = task.await;
        let Some(model) = model.upgrade() else { return };
        let mut state = model.borrow_mut();
        state.cover_pending.remove(&path);
        state.updates.request();
        let Ok(cover) = result else {
            state.cover_attempted.remove(&path);
            return;
        };
        let allow_online = state.online_covers;
        let mut retry = false;
        if let Some(track) = state.tracks.iter_mut().find(|track| track.path == path) {
            retry = cover.is_none()
                && allow_online
                && (!online
                    || (track.title.as_str(), track.artist.as_str())
                        != (lookup_identity.0.as_str(), lookup_identity.1.as_str()));
            track.cover_path = cover;
        }
        if retry {
            state.cover_attempted.remove(&path);
            drop(state);
            start_cover_job(&model, path);
        } else {
            state.cover_attempted.insert(path);
        }
    });
}

fn schedule_session_save(
    model: &Rc<RefCell<Model>>,
    pending: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    if let Some(source) = pending.borrow_mut().take() {
        source.remove();
    }
    let model = model.clone();
    let pending_done = pending.clone();
    *pending.borrow_mut() = Some(glib::timeout_add_local_once(
        Duration::from_millis(300),
        move || {
            pending_done.borrow_mut().take();
            save_session(&model);
        },
    ));
}

fn save_session(model: &Rc<RefCell<Model>>) {
    let state = model.borrow();
    let saved = Session {
        paths: state
            .tracks
            .iter()
            .map(|track| track.path.clone())
            .collect(),
        current_path: state
            .current
            .and_then(|index| state.tracks.get(index))
            .map(|track| track.path.clone()),
        play_next: state.play_next.clone(),
        shuffle: state.shuffle,
        repeat: state.repeat,
        online_covers: state.online_covers,
        muted: state.muted,
        appearance: state.appearance,
        volume: state.volume,
        speed: state.speed,
    };
    if let Err(error) = session::save(saved) {
        eprintln!("Не удалось сохранить очередь: {error}");
    }
}

fn build_settings_page(model: &Rc<RefCell<Model>>, sidebar: &gtk::Box) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_overlay_scrolling(true);
    scroll.set_shadow_type(gtk::ShadowType::None);
    scroll.style_context().add_class("settings-page");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    scroll.add(&content);
    content.set_margin_top(20);
    content.set_margin_bottom(16);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.pack_start(&settings_text("Настройки", "heading"), false, false, 0);

    let playback = settings_section(&content, "Воспроизведение");
    let speed_block = gtk::Box::new(gtk::Orientation::Vertical, 4);
    speed_block.style_context().add_class("settings-speed");
    let speed_header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    speed_header.pack_start(&settings_text("Скорость", "settings-label"), true, true, 0);
    let speed_value = settings_text(&format!("{:.2}×", model.borrow().speed), "speed-value");
    speed_value.set_xalign(0.5);
    speed_header.pack_end(&speed_value, false, false, 0);
    speed_block.pack_start(&speed_header, false, false, 0);
    let speed = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.5, 2.0, 0.05);
    speed.set_value(model.borrow().speed);
    speed.set_draw_value(false);
    let speed_save = Rc::new(RefCell::new(None));
    speed.connect_value_changed({
        let model = model.clone();
        move |scale| {
            let value = scale.value();
            {
                let mut state = model.borrow_mut();
                state.speed = value;
                state.updates.request();
                if let Some(player) = state.mpv.as_mut() {
                    let _ = player.command(json!(["set_property", "speed", value]));
                }
            }
            schedule_session_save(&model, &speed_save);
        }
    });
    describe_control(&speed, "Скорость воспроизведения");
    speed.style_context().add_class("settings-speed-slider");
    speed.connect_value_changed(move |scale| {
        speed_value.set_text(&format!("{:.2}×", scale.value()));
    });
    speed_block.pack_start(&speed, false, false, 0);

    let presets = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    presets.set_homogeneous(true);
    presets.style_context().add_class("speed-segments");
    for value in [0.75, 1.0, 1.25, 1.5, 2.0] {
        let preset = gtk::ToggleButton::with_label(&format!("{value}×"));
        preset.style_context().add_class("speed-preset");
        let matches = move |speed: f64| (speed - value).abs() < 0.001;
        preset.set_active(matches(speed.value()));
        let group_update = Rc::new(Cell::new(false));
        let slider = speed.downgrade();
        let updating = group_update.clone();
        preset.connect_toggled(move |button| {
            if updating.get() {
                return;
            }
            if let Some(slider) = slider.upgrade() {
                slider.set_value(value);
                // Clicking the current preset keeps it selected.
                if !button.is_active() {
                    button.set_active(true);
                }
            }
        });
        let button = preset.downgrade();
        speed.connect_value_changed(move |scale| {
            if let Some(button) = button.upgrade() {
                group_update.set(true);
                button.set_active(matches(scale.value()));
                group_update.set(false);
            }
        });
        presets.pack_start(&preset, true, true, 0);
    }
    speed_block.pack_start(&presets, false, false, 0);
    playback.pack_start(&speed_block, false, false, 0);

    let appearance = settings_section(&content, "Интерфейс");
    let saved = model.borrow().appearance;
    let background = settings_switch(
        &appearance,
        "Динамический фон",
        "Слегка тонировать область плеера цветом обложки.",
        saved.dynamic_background,
    );
    background.connect_active_notify({
        let model = model.clone();
        move |switch| {
            model.borrow_mut().appearance.dynamic_background = switch.is_active();
            theme::set_dynamic(switch.is_active());
            request_ui(&model);
            save_session(&model);
        }
    });
    let animations = settings_switch(
        &appearance,
        "Анимации",
        "Плавные переходы между обложками, экранами и состояниями кнопок.",
        saved.animations,
    );
    animations.connect_active_notify({
        let model = model.clone();
        move |switch| {
            model.borrow_mut().appearance.animations = switch.is_active();
            set_decorative_motion(switch.is_active());
            if let Some(window) = switch.toplevel() {
                sync_stack_motion(&window);
            }
            save_session(&model);
        }
    });
    let expanded = settings_switch(
        &appearance,
        "Расширенная боковая панель",
        "Показывать названия разделов рядом с иконками.",
        saved.expanded_sidebar,
    );
    expanded.connect_active_notify({
        let model = model.clone();
        let sidebar = sidebar.downgrade();
        move |switch| {
            model.borrow_mut().appearance.expanded_sidebar = switch.is_active();
            if let Some(sidebar) = sidebar.upgrade() {
                apply_sidebar_mode(&sidebar, switch.is_active());
            }
            save_session(&model);
        }
    });

    let library = settings_section(&content, "Библиотека");
    let (cover_row, _) = settings_row(
        "Обложки из интернета",
        "Загружать недостающие обложки из Яндекс Музыки по CSV.",
    );
    let covers = gtk::Switch::new();
    covers.set_active(model.borrow().online_covers);
    covers.set_valign(gtk::Align::Center);
    describe_control(&covers, "Загружать обложки из Яндекс Музыки");
    cover_row.pack_end(&covers, false, false, 0);
    library.pack_start(&cover_row, false, false, 0);
    let model_covers = model.clone();
    covers.connect_active_notify(move |button| {
        model_covers.borrow_mut().online_covers = button.is_active();
        save_session(&model_covers);
        if button.is_active() {
            model_covers.borrow_mut().cover_attempted.clear();
            let current = {
                let state = model_covers.borrow();
                state
                    .current
                    .and_then(|index| state.tracks.get(index).map(|track| track.path.clone()))
            };
            if let Some(path) = current {
                start_cover_job(&model_covers, path);
            }
        }
    });

    library.pack_start(
        &gtk::Separator::new(gtk::Orientation::Horizontal),
        false,
        false,
        0,
    );
    let connected = cover_art::has_index();
    let (csv_row, status) = settings_row(
        "CSV Яндекс Музыки",
        if connected {
            "Плейлист подключён"
        } else {
            "Плейлист пока не подключён"
        },
    );
    let csv_button = gtk::Button::with_label(if connected {
        "Заменить…"
    } else {
        "Выбрать…"
    });
    csv_button.set_valign(gtk::Align::Center);
    csv_button.style_context().add_class("lyrics-action");
    csv_row.pack_end(&csv_button, false, false, 0);
    library.pack_start(&csv_row, false, false, 0);
    let model_index = model.clone();
    csv_button.connect_clicked(move |button| {
        let parent = button
            .toplevel()
            .and_then(|widget| widget.downcast::<gtk::Window>().ok());
        let chooser = gtk::FileChooserDialog::with_buttons(
            Some("Выбрать CSV плейлиста"),
            parent.as_ref(),
            gtk::FileChooserAction::Open,
            &[
                ("Отмена", gtk::ResponseType::Cancel),
                ("Подключить", gtk::ResponseType::Accept),
            ],
        );
        chooser.set_modal(true);
        chooser.set_destroy_with_parent(true);
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Плейлисты CSV"));
        filter.add_pattern("*.[cC][sS][vV]");
        chooser.add_filter(filter);
        let status = status.clone();
        let button = button.clone();
        let model = model_index.clone();
        chooser.connect_response(move |chooser, response| {
            if let Some(path) = (response == gtk::ResponseType::Accept)
                .then(|| chooser.filename())
                .flatten()
            {
                match cover_art::install_index(&path) {
                    Ok(()) => {
                        status.set_text("Плейлист подключён");
                        status.style_context().remove_class("error");
                        button.set_label("Заменить…");
                        let current = {
                            let mut state = model.borrow_mut();
                            state.cover_attempted.clear();
                            state
                                .current
                                .and_then(|index| state.tracks.get(index))
                                .map(|track| track.path.clone())
                        };
                        if let Some(path) = current {
                            start_cover_job(&model, path);
                        }
                    }
                    Err(error) => {
                        status.set_text(&error);
                        status.style_context().add_class("error");
                    }
                }
            }
            chooser.close();
        });
        motion::animate_buttons(chooser.upcast_ref());
        chooser.show();
    });

    let info = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    info.style_context().add_class("settings-info");
    info.pack_start(
        &symbolize("dialog-information-symbolic", 16),
        false,
        false,
        0,
    );
    info.pack_start(
        &settings_text(
            "Если локального текста нет, плеер ищет его в LRCLIB.",
            "settings-note",
        ),
        true,
        true,
        0,
    );
    content.pack_start(&info, false, false, 0);
    scroll
}

fn settings_switch(section: &gtk::Box, title: &str, note: &str, active: bool) -> gtk::Switch {
    if !section.children().is_empty() {
        section.add(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }
    let (row, _) = settings_row(title, note);
    let control = gtk::Switch::new();
    control.set_active(active);
    control.set_valign(gtk::Align::Center);
    describe_control(&control, title);
    row.pack_end(&control, false, false, 0);
    section.add(&row);
    control
}

fn settings_text(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    label.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.style_context().add_class(class);
    label
}

fn settings_section(content: &gtk::Box, title: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.pack_start(
        &settings_text(title, "settings-section-title"),
        false,
        false,
        0,
    );
    let surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    surface.style_context().add_class("settings-section");
    section.pack_start(&surface, false, false, 0);
    content.pack_start(&section, false, false, 0);
    surface
}

fn settings_row(title: &str, note: &str) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.style_context().add_class("settings-row");
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let note = settings_text(note, "settings-note");
    labels.pack_start(&settings_text(title, "settings-label"), false, false, 0);
    labels.pack_start(&note, false, false, 0);
    row.pack_start(&labels, true, true, 0);
    (row, note)
}

fn active_window() -> Option<gtk::Window> {
    gtk::gio::Application::default()
        .and_then(|app| app.downcast::<gtk::Application>().ok())
        .and_then(|app| app.active_window())
}

fn choose_files(folder: bool) -> Option<Vec<PathBuf>> {
    let action = if folder {
        gtk::FileChooserAction::SelectFolder
    } else {
        gtk::FileChooserAction::Open
    };
    let title = if folder {
        "Добавить папку с музыкой"
    } else {
        "Добавить аудиофайлы"
    };
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some(title),
        active_window().as_ref(),
        action,
        &[
            ("Отмена", gtk::ResponseType::Cancel),
            ("Добавить", gtk::ResponseType::Accept),
        ],
    );
    if !folder {
        dialog.set_select_multiple(true);
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Аудиофайлы"));
        filter.add_mime_type("audio/*");
        for extension in [
            "mp3", "m4a", "flac", "wav", "ogg", "opus", "aac", "aiff", "ape", "oga", "wma", "webm",
        ] {
            filter.add_pattern(&format!("*.{extension}"));
            filter.add_pattern(&format!("*.{}", extension.to_uppercase()));
        }
        dialog.add_filter(filter);
        let all = gtk::FileFilter::new();
        all.set_name(Some("Все файлы"));
        all.add_pattern("*");
        dialog.add_filter(all);
    }
    let result = if dialog.run() == gtk::ResponseType::Accept {
        dialog.filenames()
    } else {
        Vec::new()
    };
    dialog.close();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn format_time(seconds: f64) -> String {
    let value = seconds.max(0.0) as u64;
    format!("{}:{:02}", value / 60, value % 60)
}

#[cfg(test)]
mod ui_tests;
