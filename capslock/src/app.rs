// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::fl;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Limits, Subscription, window::Id};
use cosmic::iced_winit::commands::popup::{destroy_popup, get_popup};
use cosmic::prelude::*;
use cosmic::widget;
use futures_util::SinkExt;
use lock_keys_protocol::LockState;

const ICON_ON: &[u8] = include_bytes!("../resources/capslockon.svg");
const ICON_OFF: &[u8] = include_bytes!("../resources/capslockoff.svg");

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
#[derive(Default)]
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// The popup id.
    popup: Option<Id>,
    /// Configuration data that persists between application runs.
    config: Config,
    /// Latest state from the daemon, `None` while it can't be reached.
    lock_state: Option<LockState>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    LockState(Option<LockState>),
    UpdateConfig(Config),
}

fn is_on(state: Option<LockState>) -> bool {
    state.is_some_and(|state| state.caps_lock)
}

/// The panel icon. Without the daemon the dimmed "off" icon is shown.
fn icon_bytes(state: Option<LockState>) -> &'static [u8] {
    if is_on(state) { ICON_ON } else { ICON_OFF }
}

fn status_text(state: Option<LockState>) -> String {
    match state {
        None => fl!("daemon-not-running"),
        Some(_) if is_on(state) => fl!("lock-on"),
        Some(_) => fl!("lock-off"),
    }
}

/// Create a COSMIC application from the app model
impl cosmic::Application for AppModel {
    /// The async executor that will be used to run your application's commands.
    type Executor = cosmic::executor::Default;

    /// Data that your application receives to its init method.
    type Flags = ();

    /// Messages which the application and its widgets will emit.
    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    const APP_ID: &'static str = "com.github.pro0grammer1.lock-keys.capslock";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Construct the app model with the runtime's core.
        let app = AppModel {
            core,
            config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
                .map(|context| match Config::get_entry(&context) {
                    Ok(config) => config,
                    Err((_errors, config)) => {
                        // for why in errors {
                        //     tracing::error!(%why, "error loading app config");
                        // }

                        config
                    }
                })
                .unwrap_or_default(),
            ..Default::default()
        };

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// The applet's button in the panel will be drawn using the main view method.
    /// This view should emit messages to toggle the applet's popup window, which will
    /// be drawn using the `view_window` method.
    fn view(&self) -> Element<'_, Self::Message> {
        let icon = widget::icon::from_svg_bytes(icon_bytes(self.lock_state));

        self.core
            .applet
            .icon_button_from_handle(icon)
            .on_press(Message::TogglePopup)
            .into()
    }

    /// The applet's popup window will be drawn using this view method. If there are
    /// multiple poups, you may match the id parameter to determine which popup to
    /// create a view for.
    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let content_list = widget::list_column()
            .padding(5)
            .spacing(0)
            .add(widget::text::body(status_text(self.lock_state)));

        self.core.applet.popup_container(content_list).into()
    }

    /// Register subscriptions for this application.
    ///
    /// Subscriptions are long-lived async tasks running in the background which
    /// emit messages to the application through a channel. They may be conditionally
    /// activated by selectively appending to the subscription batch, and will
    /// continue to execute for the duration that they remain in the batch.
    fn subscription(&self) -> Subscription<Self::Message> {
        struct DaemonSubscription;

        Subscription::batch(vec![
            // Follow the lock state sent by lock-keys-daemon.
            Subscription::run_with_id(
                std::any::TypeId::of::<DaemonSubscription>(),
                cosmic::iced::stream::channel(4, move |channel| async move {
                    let path = lock_keys_protocol::socket_path();
                    lock_keys_protocol::client::watch(&path, |state| {
                        let mut channel = channel.clone();
                        async move {
                            _ = channel.send(Message::LockState(state)).await;
                        }
                    })
                    .await;
                }),
            ),
            // Watch for application configuration changes.
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| {
                    // for why in update.errors {
                    //     tracing::error!(?why, "app config error");
                    // }

                    Message::UpdateConfig(update.config)
                }),
        ])
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime. The application will not exit until all
    /// tasks are finished.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::LockState(state) => self.lock_state = state,
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(372.0)
                        .min_width(300.0)
                        .min_height(200.0)
                        .max_height(1080.0);
                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::Application;

    const ON: LockState = LockState {
        caps_lock: true,
        num_lock: false,
    };
    // Only Num Lock is on, which this applet ignores.
    const OFF: LockState = LockState {
        caps_lock: false,
        num_lock: true,
    };

    #[test]
    fn icon_follows_caps_lock() {
        assert_eq!(icon_bytes(Some(ON)), ICON_ON);
        assert_eq!(icon_bytes(Some(OFF)), ICON_OFF);
        assert_eq!(icon_bytes(None), ICON_OFF);
    }

    #[test]
    fn status_text_covers_missing_daemon() {
        assert_eq!(status_text(Some(ON)), "Caps Lock is on");
        assert_eq!(status_text(Some(OFF)), "Caps Lock is off");
        assert_eq!(status_text(None), "lock-keys-daemon is not running");
    }

    #[test]
    fn update_stores_daemon_state() {
        let mut app = AppModel::default();
        let _ = app.update(Message::LockState(Some(ON)));
        assert_eq!(app.lock_state, Some(ON));
        let _ = app.update(Message::LockState(None));
        assert_eq!(app.lock_state, None);
    }
}
