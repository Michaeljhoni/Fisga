//! One row of the download list: a GObject whose properties are the texts shown
//! in each column, so the list cells bind to them and refresh on every update.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::glib::subclass::prelude::*;
use ldm_ipc::DownloadInfo;

use crate::format;

mod imp {
    use super::*;
    use gtk::glib::prelude::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::DownloadItem)]
    pub struct DownloadItem {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        size: RefCell<String>,
        #[property(get, set, minimum = 0.0, maximum = 1.0)]
        progress: Cell<f64>,
        #[property(get, set)]
        progress_text: RefCell<String>,
        #[property(get, set)]
        speed: RefCell<String>,
        #[property(get, set)]
        remaining: RefCell<String>,
        #[property(get, set)]
        status: RefCell<String>,
        #[property(get, set)]
        added: RefCell<String>,
        pub info: RefCell<Option<DownloadInfo>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DownloadItem {
        const NAME: &'static str = "LdmDownloadItem";
        type Type = super::DownloadItem;
    }

    #[glib::derived_properties]
    impl ObjectImpl for DownloadItem {}
}

glib::wrapper! {
    pub struct DownloadItem(ObjectSubclass<imp::DownloadItem>);
}

impl DownloadItem {
    pub fn new(info: DownloadInfo) -> Self {
        let item: Self = glib::Object::new();
        item.update(info);
        item
    }

    pub fn update(&self, info: DownloadInfo) {
        // First: a handler of the notify signals below reads the new info.
        self.imp().info.replace(Some(info.clone()));
        self.set_name(info.name.clone());
        self.set_size(format::size(&info));
        self.set_progress(format::fraction(&info));
        self.set_progress_text(format::progress_text(&info));
        self.set_speed(format::speed(&info));
        self.set_remaining(format::remaining(&info));
        self.set_status(format::status(&info));
        if self.added().is_empty() {
            self.set_added(format::added(&info));
        }
    }

    pub fn info(&self) -> DownloadInfo {
        self.imp().info.borrow().clone().expect("set in new()")
    }

    /// Reads the info without cloning it (filters run on every change).
    pub fn with_info<R>(&self, f: impl FnOnce(&DownloadInfo) -> R) -> R {
        f(self.imp().info.borrow().as_ref().expect("set in new()"))
    }
}
