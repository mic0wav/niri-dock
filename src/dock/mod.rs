mod icon_button;
mod icon_cache;
mod indicator;
mod layer_shell;

use gtk::prelude::*;
use niri_ipc_types::{Event, Window};
use relm4::prelude::*;

use icon_button::Action;

#[derive(serde::Deserialize, Clone)]
struct Launchable {
    icon: String,
    command: String,
}

#[derive(serde::Deserialize, Clone, Default)]
struct Config {
    #[serde(default)]
    launchables: std::collections::BTreeMap<String, Launchable>,
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
    NiriEvent(Event),
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
        for (name, launchable) in load_config().launchables {
            launchables.guard().push_back((
                launchable.icon,
                Action::Launch(launchable.command),
                false,
                name,
            ));
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
            let mut backoff = crate::backoff::Backoff::new(
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(30),
                std::time::Duration::from_secs(10),
            );
            loop {
                let started = std::time::Instant::now();
                if let Err(e) = crate::niri_ipc::event_stream(tx.clone()).await {
                    log::error!("Niri event stream ended: {e}");
                } else {
                    log::warn!("Niri event stream closed cleanly");
                }

                let delay = backoff.advance(started.elapsed());
                log::info!("Reconnecting to niri in {delay:?}");
                tokio::time::sleep(delay).await;
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
                if let Err(e) = sender.output(Output::Launch(x)) {
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
                Event::WindowsChanged { windows } => self.replace_all_apps(windows),
                Event::WindowOpenedOrChanged { window } if !is_own_window(&window) => {
                    self.upsert_app(&window);
                }
                Event::WindowClosed { id } => {
                    if self.focused_window == Some(id) {
                        self.focused_window = None;
                    }
                    self.remove_app(id);
                }
                Event::WindowFocusChanged { id: focused_id }
                    if focused_id != self.focused_window =>
                {
                    self.update_focus(focused_id);
                }
                _ => {}
            },
        }
    }
}

impl DockModel {
    fn replace_all_apps(&mut self, windows: Vec<Window>) {
        icon_cache::clear();
        let windows: Vec<Window> = windows.into_iter().filter(|w| !is_own_window(w)).collect();
        self.focused_window = windows.iter().find(|w| w.is_focused).map(|w| w.id);

        let mut guard = self.apps.guard();
        guard.clear();
        for w in &windows {
            guard.push_back((
                icon_cache::icon_name_for_app_id(&window_app_id(w)),
                Action::Focus(w.id),
                w.is_focused,
                window_title(w),
            ));
        }
        drop(guard);
        self.set_apps_count(windows.len());
    }

    fn upsert_app(&mut self, w: &Window) {
        let mut guard = self.apps.guard();
        match window_index(&guard, w.id) {
            Some(index) => {
                guard.send(
                    index,
                    icon_button::Input::Update {
                        icon_name: icon_cache::icon_name_for_app_id(&window_app_id(w)),
                        title: window_title(w),
                    },
                );
            }
            None => {
                guard.push_back((
                    icon_cache::icon_name_for_app_id(&window_app_id(w)),
                    Action::Focus(w.id),
                    w.is_focused,
                    window_title(w),
                ));
            }
        }
        let count = guard.len();
        drop(guard);
        self.set_apps_count(count);
    }

    fn remove_app(&mut self, id: u64) {
        let mut guard = self.apps.guard();
        if let Some(index) = window_index(&guard, id) {
            guard.remove(index);
        }
        let count = guard.len();
        drop(guard);
        self.set_apps_count(count);
    }

    fn update_focus(&mut self, focused_id: Option<u64>) {
        let guard = self.apps.guard();
        for index in 0..guard.len() {
            let id = guard.get(index).and_then(|i| i.window_id());
            if id == self.focused_window || id == focused_id {
                guard.send(index, icon_button::Input::SetFocused(id == focused_id));
            }
        }
        drop(guard);
        self.focused_window = focused_id;
    }
}

fn window_index(
    guard: &relm4::factory::AsyncFactoryVecDequeGuard<icon_button::IconButtonModel>,
    id: u64,
) -> Option<usize> {
    guard
        .iter()
        .position(|item| item.and_then(|i| i.window_id()) == Some(id))
}

fn load_config() -> Config {
    let Some(dir) = crate::config::dir() else {
        log::error!("Failed to find config directory.");
        return Config::default();
    };
    let path = dir.join("config.toml");

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) => {
            log::warn!("No config found at {}: {e}", path.display());
            return Config::default();
        }
    };

    match toml::from_str(&contents) {
        Ok(config) => config,
        Err(e) => {
            log::error!("Failed to parse config: {e}");
            Config::default()
        }
    }
}

fn is_own_window(w: &Window) -> bool {
    w.app_id.as_deref() == Some(crate::niri_ipc::APP_ID)
}

fn window_app_id(w: &Window) -> String {
    w.app_id.clone().unwrap_or_default()
}

fn window_title(w: &Window) -> String {
    w.title
        .clone()
        .or_else(|| w.app_id.clone())
        .unwrap_or_default()
}
