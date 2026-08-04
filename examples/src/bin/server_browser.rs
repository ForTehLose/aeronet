//! Standalone Steam Internet server browser.
//!
//! This queries `ISteamMatchmakingServers` directly (via `steamworks`) and
//! displays the results in an egui window - independent of any
//! `aeronet_steam` session/socket code. Useful for checking whether a
//! dedicated server (e.g. `move_box_server`) has actually registered itself
//! with Steam and is visible on the Internet server list, without relying on
//! the Steam client's own (limited) server browser UI.
//!
//! # Usage
//!
//! ```sh
//! cargo run --bin server_browser -- --app-id 480
//! ```
//!
//! This example is native-only, since `aeronet_steam`/`steamworks` don't
//! support WASM.

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        fn main() {
            panic!("not supported on WASM");
        }
    } else {

use {
    aeronet_steam::SteamworksClient,
    bevy::prelude::*,
    bevy_egui::EguiPlugin,
    examples::{
        move_box::STEAM_APP_ID,
        server_browser::{ServerBrowserConfig, ServerBrowserPlugin},
    },
};

/// Steam server browser
#[derive(Debug, Resource, clap::Parser)]
struct Args {
    /// App ID to query the Internet server list for
    #[arg(long, default_value_t = STEAM_APP_ID)]
    app_id: u32,
}

fn main() -> AppExit {
    let args = <Args as clap::Parser>::parse();

    let steam = steamworks::Client::init_app(args.app_id).expect("failed to initialize steam");

    App::new()
        .insert_resource(SteamworksClient(steam))
        .insert_resource(ServerBrowserConfig { app_id: args.app_id })
        .add_systems(PreUpdate, |steam: Res<SteamworksClient>| {
            steam.run_callbacks();
        })
        .add_plugins((DefaultPlugins, EguiPlugin::default(), ServerBrowserPlugin))
        // Required for `bevy_egui` to render content - otherwise you'll just
        // get a blank window.
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        })
        .run()
}

}}
