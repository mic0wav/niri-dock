mod app;
mod indicator;
mod launchable;

use std::{fs::File, io::Read};

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use relm4::prelude::*;

#[derive(serde::Deserialize, Clone)]
struct Launchables {
    icons: Vec<String>,
    commands: Vec<String>,
}

#[tracker::track]
pub struct DockModel {
    enabled: bool,
    visible: bool,
    #[tracker::do_not_track]
    apps: AsyncFactoryVecDeque<app::AppModel>,
    #[tracker::do_not_track]
    launchables: AsyncFactoryVecDeque<launchable::LaunchableModel>,
    #[tracker::do_not_track]
    indicator: Controller<indicator::IndicatorModel>,
    #[tracker::do_not_track]
    last_window_ids: Vec<u64>,
    apps_count: usize,
}

#[derive(Debug)]
pub enum Input {
    Init,
    Enter,
    Leave,
    Update,
    Focus(u64),
    Launch(String),
    WindowsFetched(Vec<crate::niri_ipc::WindowInfo>),
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
            #[track = "model.changed_visible() || model.changed_enabled()"]
            set_visible: model.visible && model.enabled,

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
                app::Output::Focus(x) => Input::Focus(x),
            });

        let mut launchables = AsyncFactoryVecDeque::builder()
            .launch(gtk::Box::default())
            .forward(sender.input_sender(), |msg| match msg {
                launchable::Output::Launch(x) => Input::Launch(x),
            });
        if let Some(x) = load_launchables() {
            for (i, y) in x.icons.iter().enumerate() {
                launchables
                    .guard()
                    .push_back((y.to_owned(), x.commands[i].clone()));
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
            enabled: true,
            visible: false,
            apps,
            launchables,
            indicator,
            last_window_ids: vec![],
            apps_count: 0,
            tracker: 0,
        };

        let apps_box = model.apps.widget();
        let launchables_box = model.launchables.widget();
        let widgets = view_output!();

        widgets.window.init_layer_shell();
        widgets.window.set_layer(Layer::Top);
        for (anchor, state) in [
            (Edge::Left, false),
            (Edge::Right, false),
            (Edge::Top, false),
            (Edge::Bottom, true),
        ] {
            widgets.window.set_anchor(anchor, state);
        }

        sender.input(Input::Init);
        let sender = sender.clone();
        crate::runtime().spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                sender.input(Input::Update);
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
            Input::Update => {
                let sender = sender.clone();
                crate::runtime().spawn(async move {
                    match crate::niri_ipc::get_windows().await {
                        Ok(windows) => sender.input(Input::WindowsFetched(windows)),
                        Err(e) => log::error!("Failed to get windows: {e}"),
                    }
                });
            }
            Input::Init => {
                let sender = sender.clone();
                crate::runtime().spawn(async move {
                    match crate::niri_ipc::get_windows().await {
                        Ok(windows) => sender.input(Input::WindowsFetched(windows)),
                        Err(e) => log::error!("Failed to initialize windows: {e}"),
                    }
                });
            }
            Input::WindowsFetched(windows) => {
                let ids: Vec<u64> = windows.iter().map(|w| w.id).collect();
                if ids != self.last_window_ids {
                    self.last_window_ids = ids;
                    self.set_apps_count(windows.len());
                    let mut guard = self.apps.guard();
                    guard.clear();
                    for w in windows {
                        guard.push_back((w.id, icon_name_for_app_id(&w.app_id), w.focused));
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
    let candidates = [
        format!("{app_id}.desktop"),
        format!("{}.desktop", app_id.to_lowercase()),
    ];
    for desktop_id in candidates {
        if let Some(info) = gtk::gio::DesktopAppInfo::new(&desktop_id) {
            if let Some(icon) = info.icon() {
                if let Some(name) = icon.to_string() {
                    return name.to_string();
                }
            }
        }
    }
    app_id.to_string()
}
