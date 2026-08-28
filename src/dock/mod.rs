mod icon_button;
mod indicator;
mod layer_shell;

use std::{fs::File, io::Read};

use gtk::prelude::*;
use relm4::prelude::*;

use icon_button::Action;
use crate::niri_ipc::NiriEvent;

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static ICON_CACHE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

#[derive(serde::Deserialize, Clone)]
struct Launchables {
    icons: Vec<String>,
    commands: Vec<String>,
}

#[tracker::track]
pub struct DockModel {
    visible: bool,
    #[tracker::do_not_track]
    apps: AsyncFactoryVecDeque<icon_button::IconButtonModel>,
    #[tracker::do_not_track]
    launchables: AsyncFactoryVecDeque<icon_button::IconButtonModel>,
    #[tracker::do_not_track]
    indicator: Controller<indicator::IndicatorModel>,
    #[tracker::do_not_track]
    windows: Vec<crate::niri_ipc::WindowInfo>,
    apps_count: usize,
}

#[derive(Debug)]
pub enum Input {
    Enter,
    Leave,
    Focus(u64),
    Launch(String),
    NiriEvent(NiriEvent),
}

#[derive(Debug)]
pub enum Output {
    Focus(u64),
    Launch(String),
}

impl DockModel {
    fn rebuild_apps(&mut self) {
        self.set_apps_count(self.windows.len());
        let mut guard = self.apps.guard();
        guard.clear();
        for w in &self.windows {
            guard.push_back((
                icon_name_for_app_id(&w.app_id),
                Action::Focus(w.id),
                w.focused,
                w.title.clone(),
            ));
        }
    }
}

#[relm4::component(pub)]
impl SimpleComponent for DockModel {
    type Init = ();
    type Input = Input;
    type Output = Output;

    view! {
        #[name = "window"]
        gtk::Window {
            #[track = "model.changed_visible()"]
            set_visible: model.visible,

            gtk::Box {
                set_margin_all: 8,
                set_spacing: 8,
                add_controller = gtk::EventControllerMotion {
                    connect_leave => Input::Leave,
                },

                #[local_ref]
                launchables_box -> gtk::Box {
                    add_css_class: "dock",
                    set_spacing: 8,
                },

                #[local_ref]
                apps_box -> gtk::Box {
                    #[track = "model.changed_apps_count()"]
                    set_visible: model.apps_count > 0,
                    set_spacing: 8,
                    add_css_class: "dock",
                },
            }
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let apps = AsyncFactoryVecDeque::builder()
            .launch(gtk::Box::default())
            .forward(sender.input_sender(), |msg| match msg {
                icon_button::Output::Focus(x) => Input::Focus(x),
                icon_button::Output::Launch(x) => Input::Launch(x),
            });

        let mut launchables = AsyncFactoryVecDeque::builder()
            .launch(gtk::Box::default())
            .forward(sender.input_sender(), |msg| match msg {
                icon_button::Output::Focus(x) => Input::Focus(x),
                icon_button::Output::Launch(x) => Input::Launch(x),
            });
        if let Some(x) = load_launchables() {
            if x.icons.len() != x.commands.len() {
                log::warn!(
                   "config.toml: `icons` has {} entries but `commands` has {}",
                   x.icons.len(),
                   x.commands.len()
               );
            }
            for (icon, command) in x.icons.iter().zip(x.commands.iter()) {
                launchables.guard().push_back((
                    icon.to_owned(),
                    Action::Launch(command.clone()),
                    false,
                    icon.to_owned(),
                ));
            }
        }

        let indicator_builder = indicator::IndicatorModel::builder();
        relm4::main_application().add_window(&indicator_builder.root);
        let indicator =
            indicator_builder
                .launch(())
                .forward(sender.input_sender(), |msg| match msg {
                    indicator::Output::Enter => Input::Enter,
                });

        let model = DockModel {
            visible: false,
            apps,
            launchables,
            indicator,
            windows: vec![],
            apps_count: 0,
            tracker: 0,
        };

        let apps_box = model.apps.widget();
        let launchables_box = model.launchables.widget();
        let widgets = view_output!();

        layer_shell::anchor_bottom(&widgets.window);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::runtime().spawn(async move {
            loop {
                if let Err(e) = crate::niri_ipc::event_stream(tx.clone()).await {
                    log::error!("Niri event stream ended: {e}, retrying in 2s");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });

        let sender_clone = sender.clone();
        crate::runtime().spawn(async move {
            while let Some(event) = rx.recv().await {
                sender_clone.input(Input::NiriEvent(event));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        self.reset();

        match msg {
            Input::Launch(x) => {
                sender.output(Output::Launch(x)).unwrap();
            }
            Input::Focus(x) => {
                sender.output(Output::Focus(x)).unwrap();
            }
            Input::Enter => {
                self.set_visible(true);
            }
            Input::Leave => {
                self.set_visible(false);
                self.indicator.emit(indicator::Input::Leave);
            }
            Input::NiriEvent(event) => {
                match event {
                    NiriEvent::WindowsChanged(windows) => {
                        self.windows = windows;
                        self.rebuild_apps();
                    }
                    NiriEvent::WindowOpenedOrChanged(w) => {
                        if let Some((i, existing)) = self.windows.iter_mut().enumerate().find(|(_, x)| x.id == w.id) {
                            let title_changed = existing.title != w.title;
                            let icon_changed = existing.app_id != w.app_id;

                            *existing = w;

                            if title_changed {
                                self.apps.guard().send(i, icon_button::Input::SetTitle(existing.title.clone()));
                            }
                            if icon_changed {
                                self.apps.guard().send(i, icon_button::Input::SetIcon(icon_name_for_app_id(&existing.app_id)));
                            }
                            
                        } else {
                            self.windows.push(w);
                            self.rebuild_apps();
                        }
                    }
                    NiriEvent::WindowClosed(id) => {
                        self.windows.retain(|w| w.id != id);
                        self.rebuild_apps();
                    }
                    NiriEvent::WindowFocusChanged(focused_id) => {
                        for w in self.windows.iter_mut() {
                            w.focused = Some(w.id) == focused_id;
                        }
                        for (i, w) in self.windows.iter().enumerate() {
                            self.apps.guard().send(i, icon_button::Input::SetFocused(w.focused));
                        }
                    }
                }
            }
        }
    }
}

fn load_launchables() -> Option<Launchables> {
    let Some(dir) = crate::config::dir() else {
        log::error!("Failed to find config directory.");
        return None;
    };
    let path = dir.join("config.toml");

    let mut file = match File::open(&path) {
        Ok(x) => x,
        Err(e) => {
            log::error!("Failed to read config: {e}");
            return None;
        }
    };

    let mut buf = String::new();
    match file.read_to_string(&mut buf) {
        Ok(_) => (),
        Err(e) => {
            log::error!("Failed to read config: {e}");
            return None;
        }
    }

    match toml::from_str(&buf) {
        Ok(x) => Some(x),
        Err(e) => {
            log::error!("Failed to parse config: {e}");
            None
        }
    }
}

fn icon_name_for_app_id(app_id: &str) -> String {
    if let Some(cached) = ICON_CACHE.with(|c| c.borrow().get(app_id).cloned()) {
        return cached;
    }

    let candidates = [
        format!("{app_id}.desktop"),
        format!("{}.desktop", app_id.to_lowercase()),
    ];

    let mut resolved = app_id.to_string();
    for desktop_id in candidates {
        if let Some(info) = gtk::gio::DesktopAppInfo::new(&desktop_id)
            && let Some(icon) = info.icon() 
            && let Some(name) = icon.to_string() {
                resolved = name.to_string();
                break;
        }
    }

    ICON_CACHE.with(|c| c.borrow_mut().insert(app_id.to_string(), resolved.clone()));
    resolved
}
