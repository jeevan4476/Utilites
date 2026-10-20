use crate::search::filter_clips;
use crate::storage::ClipHistory;
use crate::watcher::ClipboardWatcher;
use glib::clone;
use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CssProvider, EventControllerKey, Label,
    ListBox, ListBoxRow, Orientation, ScrolledWindow, SearchEntry, SelectionMode,
};
use std::sync::{Arc, Mutex};

pub struct ClipWindow {
    window: ApplicationWindow,
    list_box: ListBox,
    search_entry: SearchEntry,
    history: Arc<Mutex<ClipHistory>>,
}

impl ClipWindow {
    pub fn new(app: &Application, history: Arc<Mutex<ClipHistory>>) -> Self {
        load_dark_theme();

        let window = ApplicationWindow::builder()
            .application(app)
            .title("rclip")
            .default_width(380)
            .default_height(460)
            .resizable(false)
            .decorated(false)
            .build();

        window.set_size_request(380, 460);
        window.add_css_class("rclip-window");

        window.connect_close_request(|win| {
            win.set_visible(false);
            glib::Propagation::Stop
        });

        let vbox = GtkBox::new(Orientation::Vertical, 8);
        vbox.set_margin_start(10);
        vbox.set_margin_end(10);
        vbox.set_margin_top(10);
        vbox.set_margin_bottom(8);

        let search_entry = SearchEntry::builder()
            .placeholder_text("Search clipboard history...")
            .hexpand(true)
            .build();
        search_entry.add_css_class("rclip-search");
        vbox.append(&search_entry);

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .min_content_width(360)
            .min_content_height(370)
            .vexpand(true)
            .hexpand(true)
            .build();
        scrolled.add_css_class("rclip-scrolled");

        let list_box = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        list_box.add_css_class("rclip-list");
        scrolled.set_child(Some(&list_box));
        vbox.append(&scrolled);

        // Hint bar at the bottom
        let hint_box = GtkBox::new(Orientation::Horizontal, 8);
        hint_box.set_margin_top(4);
        hint_box.set_margin_bottom(2);

        let hint = Label::builder()
            .label("<span color='#787888' size='small'>↵ Paste  •  Click Select  •  Ctrl+P Pin  •  Ctrl+D Delete  •  Esc Close</span>")
            .use_markup(true)
            .xalign(0.5)
            .hexpand(true)
            .build();
        hint_box.append(&hint);
        vbox.append(&hint_box);

        window.set_child(Some(&vbox));

        let win = Self {
            window,
            list_box,
            search_entry,
            history,
        };

        win.setup_events();
        win.populate("");
        win
    }

    fn setup_events(&self) {
        let search_clone = self.search_entry.clone();
        let list_clone = self.list_box.clone();
        let hist_clone = self.history.clone();
        let win_clone = self.window.clone();

        // Real-time search change handler
        self.search_entry.connect_search_changed(clone!(
            #[strong] list_clone,
            #[strong] hist_clone,
            #[strong] win_clone,
            #[strong] search_clone,
            move |entry| {
                let query = entry.text().to_string();
                Self::repopulate_list(&list_clone, &hist_clone, &query, &win_clone, &search_clone);
            }
        ));

        // Mouse click activation: clicking any row copies and closes
        let list_act = self.list_box.clone();
        let hist_act = self.history.clone();
        let search_act = self.search_entry.clone();
        let win_act = self.window.clone();
        self.list_box.connect_row_activated(move |_, _| {
            Self::copy_selected_and_hide(&list_act, &hist_act, &search_act, &win_act);
        });

        // Keyboard navigation
        let key_controller = EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

        let win_k = self.window.clone();
        let list_k = self.list_box.clone();
        let hist_k = self.history.clone();
        let search_k = self.search_entry.clone();

        key_controller.connect_key_pressed(move |_, keyval, _, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);

            match keyval {
                gdk::Key::Escape => {
                    win_k.set_visible(false);
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    Self::copy_selected_and_hide(&list_k, &hist_k, &search_k, &win_k);
                    glib::Propagation::Stop
                }
                // Ctrl+D deletes selected row
                gdk::Key::d | gdk::Key::D if ctrl => {
                    Self::delete_selected(&list_k, &hist_k, &search_k, &win_k);
                    glib::Propagation::Stop
                }
                // Ctrl+P toggles pin on selected row
                gdk::Key::p | gdk::Key::P if ctrl => {
                    Self::toggle_pin_selected(&list_k, &hist_k, &search_k, &win_k);
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    let cur = list_k.selected_row().map(|r| r.index()).unwrap_or(-1);
                    if let Some(next_row) = list_k.row_at_index(cur + 1) {
                        list_k.select_row(Some(&next_row));
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    let cur = list_k.selected_row().map(|r| r.index()).unwrap_or(0);
                    if cur > 0 {
                        if let Some(prev_row) = list_k.row_at_index(cur - 1) {
                            list_k.select_row(Some(&prev_row));
                        }
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });

        self.window.add_controller(key_controller);
    }

    fn copy_selected_and_hide(
        list: &ListBox,
        history: &Arc<Mutex<ClipHistory>>,
        search: &SearchEntry,
        win: &ApplicationWindow,
    ) {
        if let Some(row) = list.selected_row() {
            let Ok(idx) = usize::try_from(row.index()) else { return; };
            let query = search.text().to_string();
            let hist = history.lock().unwrap();
            let filtered = filter_clips(&hist.items, &query);

            if let Some(item) = filtered.get(idx) {
                let text = item.text.clone();
                drop(hist);

                ClipboardWatcher::notify_self_copy(&text);
                if let Some(display) = gdk::Display::default() {
                    display.clipboard().set_text(&text);
                }
                win.set_visible(false);
            }
        }
    }

    fn delete_selected(
        list: &ListBox,
        history: &Arc<Mutex<ClipHistory>>,
        search: &SearchEntry,
        win: &ApplicationWindow,
    ) {
        if let Some(row) = list.selected_row() {
            let Ok(idx) = usize::try_from(row.index()) else { return; };
            let query = search.text().to_string();
            let mut hist = history.lock().unwrap();
            let filtered = filter_clips(&hist.items, &query);

            if let Some(item) = filtered.get(idx) {
                let id = item.id;
                hist.delete(id);
                let path = ClipHistory::default_storage_path();
                let _ = hist.save_to(&path);
                drop(hist);

                Self::repopulate_list(list, history, &query, win, search);
                if let Some(r) = list.row_at_index(idx as i32).or_else(|| list.row_at_index((idx.saturating_sub(1)) as i32)) {
                    list.select_row(Some(&r));
                }
            }
        }
    }

    fn toggle_pin_selected(
        list: &ListBox,
        history: &Arc<Mutex<ClipHistory>>,
        search: &SearchEntry,
        win: &ApplicationWindow,
    ) {
        if let Some(row) = list.selected_row() {
            let Ok(idx) = usize::try_from(row.index()) else { return; };
            let query = search.text().to_string();
            let mut hist = history.lock().unwrap();
            let filtered = filter_clips(&hist.items, &query);

            if let Some(item) = filtered.get(idx) {
                let id = item.id;
                hist.toggle_pin(id);
                let path = ClipHistory::default_storage_path();
                let _ = hist.save_to(&path);
                drop(hist);

                Self::repopulate_list(list, history, &query, win, search);
            }
        }
    }

    pub fn populate(&self, query: &str) {
        Self::repopulate_list(&self.list_box, &self.history, query, &self.window, &self.search_entry);
    }

    fn repopulate_list(
        list: &ListBox,
        history: &Arc<Mutex<ClipHistory>>,
        query: &str,
        win: &ApplicationWindow,
        search: &SearchEntry,
    ) {
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }

        let hist = history.lock().unwrap();
        let filtered = filter_clips(&hist.items, query);

        if filtered.is_empty() {
            let empty_box = GtkBox::new(Orientation::Vertical, 6);
            empty_box.set_margin_top(50);
            empty_box.set_margin_bottom(50);
            let empty_label = Label::builder()
                .label("<span color='#777788' size='large'>No clips found</span>")
                .use_markup(true)
                .xalign(0.5)
                .build();
            empty_box.append(&empty_label);
            list.append(&empty_box);
            return;
        }

        for item in filtered {
            let row = ListBoxRow::new();
            row.add_css_class("rclip-row");

            let hbox = GtkBox::new(Orientation::Horizontal, 8);
            hbox.set_margin_start(10);
            hbox.set_margin_end(6);
            hbox.set_margin_top(6);
            hbox.set_margin_bottom(6);

            // Clean, bounded preview (up to 2 lines, word-wrapped, newlines trimmed)
            let trimmed = item.text.trim();
            let lines: Vec<&str> = trimmed.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
            let preview = if lines.len() > 1 {
                format!("{}\n{}", lines[0], lines[1])
            } else {
                trimmed.to_string()
            };
            let preview: String = preview.chars().take(180).collect();

            let label = Label::builder()
                .label(&preview)
                .xalign(0.0)
                .yalign(0.5)
                .hexpand(true)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .lines(2)
                .wrap(true)
                .wrap_mode(gtk4::pango::WrapMode::WordChar)
                .css_classes(["rclip-clip-text"])
                .build();
            hbox.append(&label);

            // Action buttons on the right: Pin and Delete
            let actions_box = GtkBox::new(Orientation::Horizontal, 2);
            actions_box.set_valign(gtk4::Align::Center);
            actions_box.set_halign(gtk4::Align::End);

            // Pin button
            let pin_icon = if item.pinned { "★" } else { "☆" };
            let pin_class = if item.pinned { "rclip-btn-pin-active" } else { "rclip-btn" };
            let pin_btn = Button::builder()
                .label(pin_icon)
                .has_frame(false)
                .valign(gtk4::Align::Center)
                .halign(gtk4::Align::Center)
                .tooltip_text(if item.pinned { "Unpin clip (Ctrl+P)" } else { "Pin clip (Ctrl+P)" })
                .css_classes([pin_class])
                .build();
            pin_btn.set_size_request(24, 24);

            let item_id = item.id;
            let hist_pin = history.clone();
            let list_pin = list.clone();
            let win_pin = win.clone();
            let search_pin = search.clone();
            pin_btn.connect_clicked(move |_| {
                let mut h = hist_pin.lock().unwrap();
                h.toggle_pin(item_id);
                let path = ClipHistory::default_storage_path();
                let _ = h.save_to(&path);
                drop(h);
                let q = search_pin.text().to_string();
                Self::repopulate_list(&list_pin, &hist_pin, &q, &win_pin, &search_pin);
            });
            actions_box.append(&pin_btn);

            // Delete button
            let del_btn = Button::builder()
                .label("✕")
                .has_frame(false)
                .valign(gtk4::Align::Center)
                .halign(gtk4::Align::Center)
                .tooltip_text("Delete clip (Ctrl+D)")
                .css_classes(["rclip-btn", "rclip-btn-del"])
                .build();
            del_btn.set_size_request(24, 24);

            let hist_del = history.clone();
            let list_del = list.clone();
            let win_del = win.clone();
            let search_del = search.clone();
            del_btn.connect_clicked(move |_| {
                let mut h = hist_del.lock().unwrap();
                h.delete(item_id);
                let path = ClipHistory::default_storage_path();
                let _ = h.save_to(&path);
                drop(h);
                let q = search_del.text().to_string();
                Self::repopulate_list(&list_del, &hist_del, &q, &win_del, &search_del);
            });
            actions_box.append(&del_btn);

            hbox.append(&actions_box);

            row.set_child(Some(&hbox));
            list.append(&row);
        }

        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }
    }

    pub fn toggle(&self) {
        if self.window.is_visible() {
            self.window.set_visible(false);
        } else {
            self.show();
        }
    }

    pub fn show(&self) {
        self.search_entry.set_text("");
        self.populate("");
        self.window.present();
        self.search_entry.grab_focus();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn refresh(&self) {
        let query = self.search_entry.text().to_string();
        self.populate(&query);
    }
}

fn load_dark_theme() {
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
    }

    let provider = CssProvider::new();
    provider.load_from_string(
        "
        /* Modern Dark Theme for rclip */
        window.rclip-window {
            background-color: #16161e;
            color: #e2e2ec;
            border-radius: 12px;
            border: 1px solid rgba(255, 255, 255, 0.08);
            box-shadow: 0 16px 48px rgba(0, 0, 0, 0.7);
        }

        .rclip-search {
            background-color: #20202c;
            color: #ffffff;
            border: 1px solid rgba(255, 255, 255, 0.1);
            border-radius: 8px;
            padding: 8px 12px;
            font-size: 13.5px;
        }

        .rclip-search:focus-within {
            border-color: #3b82f6;
            box-shadow: 0 0 0 1px #3b82f6;
        }

        .rclip-scrolled {
            background-color: transparent;
        }

        .rclip-list {
            background-color: transparent;
        }

        .rclip-row {
            background-color: #20202a;
            color: #d8d8e2;
            border-radius: 8px;
            margin: 2px 0px;
            padding: 0px;
            border: 1px solid rgba(255, 255, 255, 0.04);
            border-left: 3px solid transparent;
            transition: background 120ms ease, border-color 120ms ease;
        }

        .rclip-row:focus,
        .rclip-row:focus-visible,
        .rclip-list > row:focus,
        .rclip-list > row:focus-visible {
            outline: none;
            box-shadow: none;
        }

        .rclip-row:hover {
            background-color: #282836;
            border-color: rgba(255, 255, 255, 0.08);
            border-left-color: rgba(255, 255, 255, 0.2);
            color: #ffffff;
        }

        .rclip-row:selected {
            background-color: #24334a;
            border-color: rgba(59, 130, 246, 0.5);
            border-left: 3px solid #3b82f6;
            color: #ffffff;
        }

        .rclip-clip-text {
            font-family: inherit;
            font-size: 13px;
            line-height: 1.4;
        }

        .rclip-row:selected .rclip-clip-text {
            color: #ffffff;
        }

        /* Generic ghost action button */
        button.rclip-btn,
        button.rclip-btn-pin-active {
            background: transparent;
            background-color: transparent;
            border: none;
            box-shadow: none;
            outline: none;
            padding: 0;
            margin: 0;
            border-radius: 4px;
            font-size: 13px;
            min-width: 24px;
            min-height: 24px;
            color: rgba(255, 255, 255, 0.35);
            transition: all 120ms ease;
        }

        button.rclip-btn:hover {
            background-color: rgba(255, 255, 255, 0.12);
            color: #ffffff;
        }

        /* Active pin styling */
        button.rclip-btn-pin-active {
            color: #f59e0b;
        }

        button.rclip-btn-pin-active:hover {
            background-color: rgba(245, 158, 11, 0.18);
            color: #fbbf24;
        }

        /* Delete hover */
        button.rclip-btn-del:hover {
            background-color: rgba(239, 68, 68, 0.2);
            color: #ef4444;
        }

        .rclip-row:selected button.rclip-btn {
            color: rgba(255, 255, 255, 0.6);
        }

        .rclip-row:selected button.rclip-btn:hover {
            background-color: rgba(255, 255, 255, 0.2);
            color: #ffffff;
        }

        .rclip-row:selected button.rclip-btn-pin-active {
            color: #fbbf24;
        }

        /* Slim dark scrollbar */
        scrollbar {
            background-color: transparent;
        }
        scrollbar slider {
            background-color: rgba(255, 255, 255, 0.12);
            border-radius: 8px;
            min-width: 6px;
            min-height: 6px;
        }
        scrollbar slider:hover {
            background-color: rgba(255, 255, 255, 0.25);
        }
        ",
    );

    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
