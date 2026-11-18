use super::*;

pub(super) fn build(model: &Rc<RefCell<Model>>) -> (gtk::Widget, gtk::Stack) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.style_context().add_class("player-stage");

    let views = gtk::Stack::new();
    views.set_hhomogeneous(false);
    views.set_vhomogeneous(false);
    views.set_transition_type(gtk::StackTransitionType::Crossfade);
    views.set_transition_duration(180);
    page.pack_start(&views, true, true, 0);

    let artwork = gtk::Overlay::new();
    // The base sets the minimum. The overlay can grow with the available area
    // without making the current artwork size a minimum for the next resize.
    let base = gtk::Box::new(gtk::Orientation::Vertical, 0);
    base.set_height_request(208);
    artwork.add(&base);
    let hero_event = gtk::EventBox::new();
    hero_event.set_visible_window(false);
    hero_event.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK);
    hero_event.set_halign(gtk::Align::Center);
    hero_event.set_valign(gtk::Align::Center);
    artwork.add_overlay(&hero_event);
    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    hero.set_halign(gtk::Align::Center);
    hero.set_valign(gtk::Align::Center);

    let cover_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    cover_frame.style_context().add_class("cover-frame");
    cover_frame.set_size_request(192, 192);
    cover_frame.set_valign(gtk::Align::Center);
    let cover_stack = gtk::Stack::new();
    cover_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    cover_stack.set_transition_duration(180);
    cover_stack.add_named(&cover_placeholder(0, 64), "placeholder");
    let (cover, picture) = cover_picture();
    cover_stack.add_named(&picture, "image");
    let (cover_alt, picture) = cover_picture();
    cover_stack.add_named(&picture, "image-alt");
    cover_stack.set_visible_child_name("placeholder");
    cover_frame.pack_start(&cover_stack, true, true, 0);
    hero.pack_start(&cover_frame, false, false, 0);

    let details = gtk::Box::new(gtk::Orientation::Vertical, 4);
    details.set_valign(gtk::Align::Center);
    details.set_width_request(180);
    details.style_context().add_class("track-details");
    let title = gtk::Label::new(Some("Добавь музыку"));
    title.set_xalign(0.0);
    title.set_line_wrap(true);
    title.set_lines(3);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_max_width_chars(1);
    title.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    title.style_context().add_class("track-title");
    let artist = gtk::Label::new(Some("Выбери файлы или добавь папку с музыкой"));
    artist.set_xalign(0.0);
    artist.set_line_wrap(true);
    artist.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    artist.set_lines(2);
    artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    artist.set_max_width_chars(1);
    artist.style_context().add_class("track-artist");
    let album = gtk::Label::new(None);
    album.set_xalign(0.0);
    album.set_ellipsize(gtk::pango::EllipsizeMode::End);
    album.set_max_width_chars(1);
    album.set_no_show_all(true);
    album.style_context().add_class("track-album");
    let show_lyrics = gtk::Button::new();
    show_lyrics.style_context().add_class("lyrics-action");
    show_lyrics.set_focus_on_click(false);
    let lyrics_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    lyrics_row.pack_start(
        &symbolize("format-justify-left-symbolic", 16),
        false,
        false,
        0,
    );
    let lyrics_label = gtk::Label::new(Some("Текст песни"));
    lyrics_row.pack_start(&lyrics_label, false, false, 0);
    show_lyrics.add(&lyrics_row);
    show_lyrics.set_halign(gtk::Align::Start);
    describe_control(&show_lyrics, "Открыть текст песни");
    let views_lyrics = views.downgrade();
    show_lyrics.connect_clicked(move |_| {
        if let Some(views) = views_lyrics.upgrade() {
            views.set_visible_child_name("lyrics");
        }
    });
    details.pack_start(&title, false, false, 0);
    details.pack_start(&artist, false, false, 0);
    details.pack_start(&album, false, false, 0);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    actions.style_context().add_class("track-actions");
    actions.set_margin_top(12);
    actions.set_no_show_all(true);
    actions.pack_start(&show_lyrics, false, false, 0);
    let track_menu = gtk::Menu::new();
    let menu_target = Rc::new(RefCell::new(PathBuf::new()));
    track_menu::add_details(&track_menu, model, &menu_target);
    track_menu.show_all();
    let more = menu_button(&track_menu, "Действия с текущим треком");
    more.style_context().add_class("icon-button");
    gtk::prelude::MenuButtonExt::set_direction(&more, gtk::ArrowType::Down);
    actions.pack_start(&more, false, false, 0);
    details.pack_start(&actions, false, false, 0);
    let context_menu = track_menu.downgrade();
    let context_target = menu_target.clone();
    hero_event.connect_button_press_event(move |_, event| {
        if event.triggers_context_menu() && !context_target.borrow().as_os_str().is_empty() {
            if let Some(menu) = context_menu.upgrade() {
                menu.popup_at_pointer(Some(event));
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    let start = gtk::Button::with_label("Добавить музыку");
    start.style_context().add_class("tonal-button");
    start.set_halign(gtk::Align::Start);
    start.set_margin_top(22);
    let model_start = model.clone();
    start.connect_clicked(move |_| {
        if let Some(paths) = choose_files(false) {
            append_paths(&model_start, paths);
        }
    });
    details.pack_start(&start, false, false, 0);
    start.show_all();
    actions.show_all();
    more.show_all();
    show_lyrics.show_all();
    start.set_no_show_all(true);

    start.set_visible(model.borrow().current.is_none());
    actions.set_visible(model.borrow().current.is_some());
    // Keep the centered group in place as metadata changes between tracks.
    hero.pack_start(&details, false, false, 0);
    hero_event.add(&hero);
    base.connect_size_allocate({
        let hero = hero.downgrade();
        let cover = cover_frame.downgrade();
        let details = details.downgrade();
        move |_, allocation| {
            let (Some(hero), Some(cover), Some(details)) =
                (hero.upgrade(), cover.upgrade(), details.upgrade())
            else {
                return;
            };
            let width = allocation.width();
            let height = allocation.height();
            let compact = width < 680 || height < 370;
            let spacing = if compact { 16 } else { 32 };
            let details_width = (width * 2 / 5).clamp(180, 360);
            let edge = (width - details_width - spacing - 32)
                .min(height - 44)
                .clamp(112, 376);
            if cover.width_request() != edge {
                cover.set_size_request(edge, edge);
            }
            if details.width_request() != details_width {
                details.set_width_request(details_width);
            }
            if hero.spacing() != spacing {
                hero.set_spacing(spacing);
            }
            set_style_class(&hero, "compact-hero", compact);
        }
    });
    views.add_named(&artwork, "artwork");
    views.add_named(&build_lyrics_view(model, &views), "lyrics");
    views.set_visible_child_name("artwork");
    let mapped = Rc::new(Cell::new(false));
    views.connect_map(move |stack| {
        if !mapped.replace(true) {
            stack.set_visible_child_name("artwork");
        }
    });

    let model_update = model.clone();
    let views_update = views.downgrade();
    let updates = model.borrow().updates.clone();
    let metadata_fade = motion::Tween::new(1.0);
    details.connect_unmap({
        let fade = metadata_fade.clone();
        move |details| {
            fade.snap(1.0);
            details.set_opacity(1.0);
        }
    });
    let mut rendered: Option<(PathBuf, String, String, String)> = None;
    let mut rendered_cover = None;
    updates.listen(&artwork, move || {
        let state = model_update.borrow();
        if let Some(index) = state.current {
            if let Some(track) = state.tracks.get(index) {
                if rendered
                    .as_ref()
                    .map(|(path, title, artist, album)| (path, title, artist, album))
                    != Some((&track.path, &track.title, &track.artist, &track.album))
                {
                    if rendered
                        .as_ref()
                        .is_some_and(|(path, _, _, _)| *path != track.path)
                        && details.is_mapped()
                        && animations_enabled()
                    {
                        metadata_fade.snap(0.82);
                        details.set_opacity(0.82);
                        metadata_fade.animate(&details, 1.0, 170, |widget, opacity| {
                            widget.set_opacity(opacity)
                        });
                    }
                    rendered = Some((
                        track.path.clone(),
                        track.title.clone(),
                        track.artist.clone(),
                        track.album.clone(),
                    ));
                    start.hide();
                    actions.show();
                    menu_target.replace(track.path.clone());
                    title.set_text(&track.title);
                    title.set_tooltip_text(Some(&track.title));
                    artist.set_text(if track.artist.is_empty() {
                        "Неизвестный исполнитель"
                    } else {
                        &track.artist
                    });
                    artist.set_tooltip_text(Some(&track.artist));
                    album.set_text(&track.album);
                    album.set_tooltip_text(Some(&track.album));
                    album.set_visible(!track.album.is_empty());
                }
                let lyrics_status = if !track.lyrics.is_empty() {
                    ("Текст песни", "Открыть текст песни")
                } else if state.lyrics_pending.contains(&track.path) {
                    ("Текст песни", "Ищем текст песни")
                } else if state.lyrics_results.contains_key(&track.path) {
                    ("Нет текста", "Открыть текст песни и повторить поиск")
                } else {
                    ("Текст песни", "Открыть текст песни")
                };
                if lyrics_label.text() != lyrics_status.0 {
                    lyrics_label.set_text(lyrics_status.0);
                }
                if show_lyrics.tooltip_text().as_deref() != Some(lyrics_status.1) {
                    describe_control(&show_lyrics, lyrics_status.1);
                }
                if let Some(image) = current_artwork(track, &mut rendered_cover, &state) {
                    let image = image.as_ref().and_then(|art| art.hero.as_ref());
                    if let Some(image) = image {
                        let (slot, name) =
                            if cover_stack.visible_child_name().as_deref() == Some("image") {
                                (&cover_alt, "image-alt")
                            } else {
                                (&cover, "image")
                            };
                        slot.replace(Some(image.clone()));
                        cover_stack.set_visible_child_name(name);
                    } else {
                        cover_stack.set_visible_child_name("placeholder");
                    }
                }
            }
        }
        if state.current.is_none() && rendered.take().is_some() {
            title.set_text("Добавь музыку");
            title.set_tooltip_text(None);
            artist.set_text("Выбери файлы или добавь папку с музыкой");
            artist.set_tooltip_text(None);
            album.hide();
            actions.hide();
            menu_target.replace(PathBuf::new());
            start.show();
            rendered_cover = None;
            cover_stack.set_visible_child_name("placeholder");
            if let Some(views) = views_update.upgrade() {
                views.set_visible_child_name("artwork");
            }
        }
    });
    (page.upcast(), views)
}

type CoverSurface = Rc<RefCell<Option<artwork::HeroImage>>>;

fn cover_picture() -> (CoverSurface, gtk::DrawingArea) {
    let area = gtk::DrawingArea::new();
    let surface: CoverSurface = Rc::new(RefCell::new(None));
    let source = surface.clone();
    area.connect_draw(move |area, cr| {
        if let Some(image) = source.borrow().as_ref() {
            let _ = cr.save();
            let width = f64::from(area.allocated_width());
            let height = f64::from(area.allocated_height());
            let radius = 16.0;
            let half_pi = std::f64::consts::FRAC_PI_2;
            cr.new_sub_path();
            cr.arc(width - radius, radius, radius, -half_pi, 0.0);
            cr.arc(width - radius, height - radius, radius, 0.0, half_pi);
            cr.arc(radius, height - radius, radius, half_pi, 2.0 * half_pi);
            cr.arc(radius, radius, radius, 2.0 * half_pi, 3.0 * half_pi);
            cr.close_path();
            cr.clip();
            cr.scale(width / image.edge, height / image.edge);
            if cr.set_source_surface(&image.surface, 0.0, 0.0).is_ok() {
                cr.source().set_filter(gtk::cairo::Filter::Good);
                let _ = cr.paint();
            }
            let _ = cr.restore();
        }
        glib::Propagation::Proceed
    });
    (surface, area)
}
