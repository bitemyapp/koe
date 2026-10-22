//! StatusNotifierItem + DBusMenu, consumed directly by Aizome's systray.
//! Menu actions use the same serialized daemon protocol as keyboard shortcuts.
use crate::{
    desktop, ipc,
    protocol::{Action, Phase, Request, Status},
};
use ksni::{TrayMethods, menu::StandardItem};
use std::{path::PathBuf, time::Duration};
use tokio::sync::{mpsc, watch};

struct Tray {
    status: Status,
    actions: mpsc::Sender<Action>,
    error: Option<String>,
}

impl Tray {
    fn label(&self) -> &'static str {
        match self.status.phase {
            Phase::Idle => "Ready",
            Phase::Recording => "Listening",
            Phase::Paused => "Paused",
            Phase::Finishing => "Stopping microphone",
            Phase::Transcribing => "Transcribing",
            Phase::Delivering => "Pasting",
            Phase::Error => "Needs attention",
        }
    }
    fn item(&self, label: &str, action: Action, enabled: bool) -> ksni::MenuItem<Self> {
        StandardItem {
            label: label.into(),
            enabled,
            activate: Box::new(move |tray: &mut Self| {
                let _ = tray.actions.try_send(action);
            }),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for Tray {
    // Opening the tray is never an implicit request to open the microphone.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "org.aizome.Koe".into()
    }
    fn title(&self) -> String {
        format!("Koe — {}", self.label())
    }
    fn status(&self) -> ksni::Status {
        if self.status.phase == Phase::Error || self.error.is_some() {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![icon(self.status.phase)]
    }
    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![icon(Phase::Error)]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let description = self
            .error
            .as_ref()
            .or(self.status.error.as_ref())
            .cloned()
            .unwrap_or_else(|| match self.status.phase {
                Phase::Recording => "Microphone on. Stop dictation to transcribe.".into(),
                Phase::Paused => "Microphone off. Resume or stop dictation.".into(),
                Phase::Finishing | Phase::Transcribing | Phase::Delivering => {
                    "Finishing dictation. You can cancel from the menu.".into()
                }
                _ => "Super+Alt+D to dictate. Click for controls.".into(),
            });
        ksni::ToolTip {
            title: self.title(),
            description,
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let phase = self.status.phase;
        let idle = matches!(phase, Phase::Idle | Phase::Error);
        let recording = matches!(phase, Phase::Recording | Phase::Paused);
        vec![
            StandardItem {
                label: self.title(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            self.item(
                if recording {
                    "Stop dictation"
                } else {
                    "Start dictation"
                },
                Action::Toggle,
                idle || recording,
            ),
            self.item(
                if phase == Phase::Paused {
                    "Resume dictation"
                } else {
                    "Pause dictation"
                },
                if phase == Phase::Paused {
                    Action::Resume
                } else {
                    Action::Pause
                },
                recording,
            ),
            self.item("Cancel dictation", Action::Cancel, !idle),
            ksni::MenuItem::Separator,
            self.item(
                "Copy last transcript",
                Action::CopyLast,
                self.status.has_last,
            ),
            self.item(
                "Forget last transcript",
                Action::ClearLast,
                self.status.has_last,
            ),
            self.item(
                "Unload model",
                Action::Unload,
                idle && self.status.model_loaded,
            ),
            ksni::MenuItem::Separator,
            self.item("Quit Koe", Action::Shutdown, true),
        ]
    }
}

// Embed a small, crisp microphone as SNI's network-order ARGB pixels, so tray
// hosts do not need an installed icon theme. Recording is red, busy amber,
// paused blue, errors orange, and idle cream. No raster asset dependency.
fn icon(phase: Phase) -> ksni::Icon {
    let color = match phase {
        Phase::Recording => [240, 90, 90],
        Phase::Paused => [117, 177, 229],
        Phase::Finishing | Phase::Transcribing | Phase::Delivering => [232, 177, 91],
        Phase::Error => [255, 135, 75],
        Phase::Idle => [242, 237, 227],
    };
    let mut argb = vec![0; 32 * 32 * 4];
    for y in 0i32..32 {
        for x in 0i32..32 {
            let capsule = (12..20).contains(&x)
                && (6..18).contains(&y)
                && ((9..15).contains(&y)
                    || (x - 16).pow(2) + (y - if y < 9 { 9 } else { 14 }).pow(2) <= 16);
            let ring = (8..24).contains(&x)
                && (14..24).contains(&y)
                && (49..=64).contains(&((x - 16).pow(2) + (y - 15).pow(2)));
            let stem = (15..18).contains(&x) && (22..27).contains(&y);
            let foot = (11..22).contains(&x) && (26..28).contains(&y);
            if capsule || ring || stem || foot {
                let offset = ((y * 32 + x) * 4) as usize;
                argb[offset..offset + 4].copy_from_slice(&[255, color[0], color[1], color[2]]);
            }
        }
    }
    ksni::Icon {
        width: 32,
        height: 32,
        data: argb,
    }
}

pub async fn run(path: PathBuf, mut updates: watch::Receiver<Status>, notify: bool) {
    let (actions, mut commands) = mpsc::channel(8);
    let mut previous = updates.borrow_and_update().clone();
    let tray = Tray {
        status: previous.clone(),
        actions,
        error: None,
    };
    // Keep the service alive if the shell starts later or restarts.
    let handle = match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("Koe tray unavailable: {error}; dictation shortcuts remain available");
            return;
        }
    };
    loop {
        tokio::select! {
            result = updates.changed() => {
                if result.is_err() { break; }
                let status = updates.borrow_and_update().clone();
                // Meter updates are 10 Hz; SNI only needs state changes.
                if previous.phase == status.phase && previous.has_last == status.has_last
                    && previous.model_loaded == status.model_loaded && previous.error == status.error {
                    continue;
                }
                previous = status.clone();
                handle.update(|tray| {
                    if tray.status.phase != status.phase { tray.error = None; }
                    tray.status = status;
                }).await;
            }
            Some(action) = commands.recv() => {
                // Let the shell dismiss the menu and restore application focus.
                tokio::time::sleep(Duration::from_millis(200)).await;
                let result = ipc::request(&path, &Request::new(action)).await;
                let error = match result {
                    Ok(response) if response.ok => None,
                    Ok(response) => response.error,
                    Err(error) => Some(format!("{error:#}")),
                };
                if let Some(ref message) = error {
                    eprintln!("Koe tray: {message}");
                    if notify { desktop::notify("Koe needs attention", message.clone()); }
                }
                handle.update(|tray| tray.error = error).await;
            }
        }
    }
    handle.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    #[tokio::test]
    async fn tray_tracks_phase_and_routes_controls_without_transcript_text() {
        let (actions, mut commands) = mpsc::channel(8);
        let mut tray = Tray {
            status: Status::default(),
            actions,
            error: None,
        };
        assert_eq!(tray.title(), "Koe — Ready");
        let mut menu = tray.menu();
        let ksni::MenuItem::Standard(start) = &mut menu[2] else {
            panic!("start missing")
        };
        assert!(start.enabled);
        (start.activate)(&mut tray);
        assert_eq!(commands.recv().await, Some(Action::Toggle));
        tray.status.phase = Phase::Transcribing;
        assert!(tray.tool_tip().description.contains("Finishing"));
        let menu = tray.menu();
        let ksni::MenuItem::Standard(start) = &menu[2] else {
            panic!()
        };
        let ksni::MenuItem::Standard(cancel) = &menu[4] else {
            panic!()
        };
        assert!(!start.enabled && cancel.enabled);
        assert_ne!(icon(Phase::Idle).data, icon(Phase::Recording).data);
        assert_eq!(tray.icon_pixmap()[0].data.len(), 32 * 32 * 4);
    }
}
