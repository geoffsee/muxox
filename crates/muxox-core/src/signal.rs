// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use crate::app::AppMsg;
use tokio::sync::mpsc;

#[cfg(unix)]
pub async fn signal_watcher(tx: mpsc::UnboundedSender<AppMsg>) {
    use tokio::signal::unix::{SignalKind, signal};

    // Create and pin futures
    let ctrlc = tokio::signal::ctrl_c();
    let sigterm = signal(SignalKind::terminate()).expect("sigterm");

    tokio::pin!(ctrlc, sigterm);

    // For Unix systems, wait for either Ctrl-C or SIGTERM
    #[allow(clippy::never_loop)]
    loop {
        tokio::select! {
            _ = &mut ctrlc => { let _=tx.send(AppMsg::AbortedAll); break; }
            _ = sigterm.recv() => { let _=tx.send(AppMsg::AbortedAll); break; }
        }
    }
}

#[cfg(not(unix))]
pub async fn signal_watcher(tx: mpsc::UnboundedSender<AppMsg>) {
    // Create and pin futures
    let ctrlc = tokio::signal::ctrl_c();
    tokio::pin!(ctrlc);

    // For non-Unix systems, just wait for Ctrl-C
    loop {
        tokio::select! {
            _ = &mut ctrlc => { let _=tx.send(AppMsg::AbortedAll); break; }
        }
    }
}
