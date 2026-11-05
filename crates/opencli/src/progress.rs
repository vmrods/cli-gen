//! A spinner on stderr while slow work runs, only when a person is watching:
//! nothing is drawn when stderr isn't a terminal, so scripts and pipes see
//! nothing.

use std::future::Future;
use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

const TICK: Duration = Duration::from_millis(100);
const TEMPLATE: &str = "{spinner} {msg} {elapsed}";

pub async fn with_spinner<T>(message: impl Into<String>, work: impl Future<Output = T>) -> T {
    if !std::io::stderr().is_terminal() {
        return work.await;
    }
    let spinner = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
    if let Ok(style) = ProgressStyle::with_template(TEMPLATE) {
        spinner.set_style(style);
    }
    spinner.set_message(message.into());
    spinner.enable_steady_tick(TICK);
    let result = work.await;
    spinner.finish_and_clear();
    result
}
