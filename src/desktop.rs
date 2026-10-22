use crate::{
    config::{Desktop, Output, OutputMode},
    ipc,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{env, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

pub fn aizome_socket() -> Option<PathBuf> {
    env::var_os("AIZOME_SOCKET").map(PathBuf::from)
}
pub fn uses_aizome(output: &Output) -> bool {
    output.desktop == Desktop::Aizome
        || (output.desktop == Desktop::Auto && aizome_socket().is_some())
}
async fn compositor(value: Value) -> Result<Value> {
    let path = aizome_socket().context("AIZOME_SOCKET is unset")?;
    let response: Value = ipc::exchange(&path, &value).await?;
    ensure!(
        response["response"] != "error",
        "Aizome rejected the request"
    );
    Ok(response)
}
pub async fn focused_window() -> Result<Option<u64>> {
    let value = compositor(json!({"request":"focused-window"})).await?;
    ensure!(
        value["response"] == "window",
        "unexpected Aizome focus response"
    );
    Ok(value["window"]["id"].as_u64())
}
async fn can_paste(target: Option<u64>, guard: bool) -> Result<()> {
    let input = compositor(json!({"request":"input-state"})).await?;
    ensure!(
        input["response"] == "input-state",
        "unexpected Aizome input response"
    );
    ensure!(
        input["state"]["pressed_keys"]
            .as_array()
            .is_some_and(|k| k.is_empty()),
        "release keyboard modifiers before pasting; use koe record paste-last"
    );
    let recovery = compositor(json!({"request":"recovery-state"})).await?;
    ensure!(
        recovery["state"]["locked"] == false,
        "desktop is locked; transcript remains in memory"
    );
    if guard {
        ensure!(
            target.is_some() && focused_window().await? == target,
            "focused window changed; use koe record paste-last in the intended window"
        );
    }
    Ok(())
}
pub async fn copy(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("cannot start wl-copy; install wl-clipboard")?;
    let mut stdin = child.stdin.take().context("wl-copy has no stdin")?;
    tokio::time::timeout(Duration::from_secs(5), async {
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
        let status = child.wait().await?;
        ensure!(
            status.success(),
            "wl-copy failed; check the Wayland session"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("clipboard operation timed out")?
}
pub async fn deliver(text: &str, output: &Output, target: Option<u64>) -> Result<()> {
    if output.mode == OutputMode::None {
        return Ok(());
    }
    if output.mode == OutputMode::Paste && uses_aizome(output) {
        // Check before changing the clipboard, then again immediately before input.
        can_paste(target, output.require_same_window).await?;
    }
    copy(text).await?;
    if output.mode == OutputMode::Clipboard {
        return Ok(());
    }
    tokio::time::sleep(Duration::from_millis(output.paste_delay_ms)).await;
    if uses_aizome(output) {
        can_paste(target, output.require_same_window).await?;
        let reply = compositor(
            json!({"request":"synthesize","events":[{"type":"chord","keys":"super+v"}]}),
        )
        .await?;
        ensure!(
            reply["response"] == "ok",
            "unexpected Aizome paste response"
        );
    } else {
        let mut child = Command::new("wtype")
            .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("cannot start wtype; install it or use output.mode = 'clipboard'")?;
        let result = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .context("wtype timed out")??;
        ensure!(
            result.success(),
            "wtype failed; compositor may not support virtual keyboards; use clipboard mode"
        );
    }
    Ok(())
}
pub fn notify(summary: &'static str, body: impl Into<String>) {
    let body = body.into();
    tokio::spawn(async move {
        if let Some(path) = env::var_os("AIZOME_SHELL_SOCKET") {
            let _: Result<Value> = ipc::exchange(&PathBuf::from(path), &json!({"type":"request","request":"notify","app":"Koe","summary":summary,"body":body})).await;
        }
    });
}
