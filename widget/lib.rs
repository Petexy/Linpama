//! Native port of the Package Manager widget (`y-package_manager.py`, linpama),
//! installed as `/usr/share/linexin/widgets/y-package_manager.so`.
//!
//! Two front ends share what is asked of pacman, the AUR and the Arch Wiki
//! (`pacman`, `http`, `wiki`, `runner`): the GTK page (`ui`, identical to the
//! Python widget) and the LineXinBar page (`lxb`).

mod http;
mod lxb;
mod pacman;
mod runner;
mod ui;
mod wiki;

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use gtk::glib::translate::{FromGlibPtrNone, IntoGlibPtr};
use gtk::prelude::*;
use linexin_plugin_api as api;

static WIDGET: api::WidgetDescriptorV1 = api::WidgetDescriptorV1::new(
    c"package_manager",
    c"Package Manager",
    c"/usr/share/icons/github.petexy.linpama.svg",
    Some(create),
)
.with_lxb_page(c"category-system", lxb::create, lxb::page, lxb::destroy);
api::export_widget!(WIDGET);

unsafe extern "C" fn create(context: *const api::CreateContextV1) -> *mut c_void {
    let widget = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `context` is the pointer the host passed to `create`.
        let context = unsafe { api::CreateContextV1::read(context) }?;
        if let Err(error) = adw::init() {
            eprintln!("Package Manager: could not initialize GTK: {error}");
            return None;
        }
        let window = (!context.window.is_null())
            .then(|| {
                // SAFETY: the host passes a live GtkWindow*; a new reference is taken.
                let object = unsafe {
                    gtk::glib::Object::from_glib_none(
                        context.window.cast::<gtk::glib::gobject_ffi::GObject>(),
                    )
                };
                object.downcast::<gtk::Window>().ok()
            })
            .flatten();
        let root = ui::build(api::host(), window).upcast::<gtk::Widget>();
        let widget: *mut gtk::ffi::GtkWidget = root.into_glib_ptr();
        Some(widget.cast::<c_void>())
    }));
    match widget {
        Ok(Some(widget)) => widget,
        Ok(None) => ptr::null_mut(),
        Err(_) => {
            eprintln!("Package Manager: widget creation panicked");
            ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn descriptor_matches_the_python_widget() {
        let text = |pointer| unsafe { CStr::from_ptr(pointer) }.to_str().unwrap();
        assert_eq!(text(WIDGET.widget_id), ui::DOMAIN);
        assert_eq!(text(WIDGET.name), "Package Manager");
        assert_eq!(
            text(WIDGET.icon),
            "/usr/share/icons/github.petexy.linpama.svg"
        );
        assert_eq!(text(WIDGET.lxb_mark), "category-system");
        assert!(WIDGET.create.is_some() && WIDGET.lxb_page.is_some());
    }

    #[test]
    fn entry_point_rejects_a_missing_host() {
        assert!(unsafe { linexin_widget_v1(ptr::null()) }.is_null());
        assert!(unsafe { create(ptr::null()) }.is_null());
    }
}
