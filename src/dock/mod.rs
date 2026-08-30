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
    focused_window: Option<u64>,
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
            apps_count: 0,
            tracker: 0,
            focused_window: None,
        };

        let apps_box = model.apps.widget();
        let launchables_box = model.launchables.widget();
        let widgets = view_output!();

        layer_shell::anchor_bottom(&widgets.window);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::runtime().spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(30);
            loop {
                let started = std::time::Instant::now();
                if let Err(e) = crate::niri_ipc::event_stream(tx.clone()).await {
                    log::error!("Niri event stream ended: {e}");
                } else {
                    log::warn!("Niri event stream closed cleanly");
                }

                backoff = if started.elapsed() > std::time::Duration::from_secs(10) {
                    std::time::Duration::from_secs(1)
                } else {
                    std::cmp::min(backoff * 2, max_backoff)
                };
                log::info!("Reconnecting to niri in {backoff:?}");
                tokio::time::sleep(backoff).await;
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
                if let Err(e) =  sender.output(Output::Launch(x)) {
                    log::error!("Failed to forward launch output: {e:?}");
                }
            }
            Input::Focus(x) => {
                if let Err(e) = sender.output(Output::Focus(x)) {
                    log::error!("Failed to to forward focus output: {e:?}");
                }
            }
            Input::Enter => {
                self.set_visible(true);
            }
            Input::Leave => {
                self.set_visible(false);
                self.indicator.emit(indicator::Input::Leave);
            }
            Input::NiriEvent(event) => match event {
                NiriEvent::WindowsChanged(windows) => {
                    self.focused_window = windows.iter().find(|w| w.focused).map(|w| w.id);
                    let mut guard = self.apps.guard();
                    guard.clear();
                    for w in &windows {
                        guard.push_back((
                            icon_name_for_app_id(&w.app_id),
                            Action::Focus(w.id),
                            w.focused,
                            w.title.clone(),
                        ));
                    }
                    drop(guard);
                    self.set_apps_count(windows.len());
                }
                NiriEvent::WindowOpenedOrChanged(w) => {
                    let mut guard = self.apps.guard();
                    let existing = guard.iter().position(|item| item.and_then(|i| i.window_id()) == Some(w.id));
                    match existing {
                        Some(index) => {
                            guard.send(index, icon_button::Input::Update {
                                icon_name: icon_name_for_app_id(&w.app_id),
                                title: w.title,
                            });
                        }
                        None => {
                            guard.push_back((
                                icon_name_for_app_id(&w.app_id),
                                Action::Focus(w.id),
                                w.focused,
                                w.title,
                            ));
                        }
                    }
                    let count = guard.len();
                    drop(guard);
                    self.set_apps_count(count);
                }
                NiriEvent::WindowClosed(id) => {
                    if self.focused_window == Some(id) {
                        self.focused_window = None;
                    }
                    let mut guard = self.apps.guard();
                    let index = guard.iter()
                        .position(|item| item.and_then(|i| i.window_id()) == Some(id));
                    if let Some(index) = index {
                        guard.remove(index);
                    }
                    let count = guard.len();
                    drop(guard);
                    self.set_apps_count(count);
                }
                NiriEvent::WindowFocusChanged(focused_id) => {
                    if focused_id != self.focused_window {
                        let guard = self.apps.guard();
                        for index in 0..guard.len() {
                            let id = guard.get(index).and_then(|i| i.window_id());
                            if id == self.focused_window || id == focused_id {
                                guard.send(index, icon_button::Input::SetFocused(id == focused_id));
                            }
                        }
                        self.focused_window = focused_id;
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
            log::warn!("No config found at {}: {e}", path.display());
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

    for desktop_id in candidates {
        if let Some(info) = gtk::gio::DesktopAppInfo::new(&desktop_id)
            && let Some(icon) = info.icon()
            && let Some(name) = icon.to_string()
        {
            let resolved = name.to_string();
            ICON_CACHE.with(|c| c.borrow_mut().insert(app_id.to_string(), resolved.clone()));
            return resolved;
        }
    }
    app_id.to_string()
}
