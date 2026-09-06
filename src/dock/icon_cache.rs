use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CACHE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

pub fn clear() {
    CACHE.with(|c| c.borrow_mut().clear());
}

pub fn icon_name_for_app_id(app_id: &str) -> String {
    if let Some(cached) = CACHE.with(|c| c.borrow().get(app_id).cloned()) {
        return cached;
    }

    if app_id.is_empty() {
        return "application-x-executable-symbolic".to_string();
    }

    let candidates = [
        format!("{app_id}.desktop"),
        format!("{}.desktop", app_id.to_lowercase()),
    ];

    let mut resolved = app_id.to_string();
    for desktop_id in candidates {
        if let Some(info) = gtk4::gio::DesktopAppInfo::new(&desktop_id)
            && let Some(icon) = info.icon()
            && let Some(name) = icon.to_string()
        {
            resolved = name.to_string();
            break;
        }
    }

    CACHE.with(|c| c.borrow_mut().insert(app_id.to_string(), resolved.clone()));
    resolved
}
