//! Bevy plugin which queries and displays Steam server lists (both Internet
//! and LAN) for a given app ID, using the `ISteamMatchmakingServers`
//! interface (exposed by the `steamworks` crate, which `aeronet_steam`
//! depends on internally).
//!
//! This is a debugging/diagnostic tool - it lets you check whether a
//! dedicated server has actually registered itself with Steam's master
//! server and is visible the same way the Steam client's own server browser
//! sees it, without having to rely on the Steam client UI at all.
//!
//! # Usage
//!
//! Add [`ServerBrowserPlugin`] to your app, and insert a
//! [`ServerBrowserConfig`] resource specifying which app ID to query. You
//! must already have an [`aeronet_steam::SteamworksClient`] resource present
//! (any of `aeronet_steam`'s client/server/dedicated-server setups insert
//! one), and `bevy_egui`'s `EguiPlugin` must be added.

use {
    aeronet_steam::SteamworksClient,
    bevy::prelude::*,
    bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui},
    core::net::SocketAddrV4,
    std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    },
    steamworks::{GameServerItem, ServerListCallbacks, ServerListRequest, ServerResponse},
};

/// Adds a "Server Browser" egui window which queries the Internet and LAN
/// server lists for [`ServerBrowserConfig::app_id`].
///
/// See the module docs for setup requirements.
pub struct ServerBrowserPlugin;

impl Plugin for ServerBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ServerBrowserState>()
            .add_systems(EguiPrimaryContextPass, server_browser_ui);
    }
}

/// Which Steam app ID [`ServerBrowserPlugin`] should query server lists for.
#[derive(Debug, Clone, Copy, Resource)]
pub struct ServerBrowserConfig {
    /// App ID to query servers for.
    pub app_id: u32,
}

/// A single server entry, as reported by Steam's matchmaking servers.
#[derive(Debug, Clone)]
pub struct BrowserServer {
    /// Name the server reports.
    pub name: String,
    /// Current map, if reported.
    pub map: String,
    /// Game directory / mod dir the server reports.
    pub game_dir: String,
    /// Human players currently connected.
    pub players: i32,
    /// Maximum player capacity the server reports.
    pub max_players: i32,
    /// Bot players currently connected.
    pub bots: i32,
    /// Round-trip ping to the server, as measured by Steam.
    pub ping_ms: u64,
    /// Steam ID of the server itself (not any connected player).
    pub steam_id: u64,
    /// Address clients should connect to.
    pub addr: SocketAddrV4,
    /// Whether the server is password-protected.
    pub has_password: bool,
    /// Whether the server has VAC / anti-cheat enabled.
    pub secure: bool,
}

impl From<GameServerItem> for BrowserServer {
    fn from(item: GameServerItem) -> Self {
        Self {
            name: item.server_name,
            map: item.map,
            game_dir: item.game_dir,
            players: item.players,
            max_players: item.max_players,
            bots: item.bot_players,
            #[expect(
                clippy::cast_possible_truncation,
                reason = "ping should never realistically overflow a u64 of milliseconds"
            )]
            ping_ms: item.ping.as_millis() as u64,
            steam_id: item.steamid,
            addr: SocketAddrV4::new(item.addr, item.connection_port),
            has_password: item.have_password,
            secure: item.secure,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum QueryStatus {
    #[default]
    Idle,
    Refreshing,
    Done {
        found: usize,
    },
}

/// State for a single, independent server-list query (e.g. Internet or LAN).
struct ServerQuery {
    servers: Arc<Mutex<Vec<BrowserServer>>>,
    status: Arc<Mutex<QueryStatus>>,
    // kept only so `refresh` can bail out early if a query is still in
    // flight; each request releases itself once `refresh_complete` fires
    request: Option<Arc<Mutex<ServerListRequest>>>,
}

impl Default for ServerQuery {
    fn default() -> Self {
        Self {
            servers: Arc::new(Mutex::new(Vec::new())),
            status: Arc::new(Mutex::new(QueryStatus::default())),
            request: None,
        }
    }
}

impl ServerQuery {
    fn is_refreshing(&self) -> bool {
        self.request
            .as_ref()
            .is_some_and(|request| request.lock().unwrap().is_refreshing().unwrap_or(false))
    }

    /// Builds a fresh set of callbacks which feed results back into this
    /// query's `servers`/`status`, and marks this query as refreshing.
    ///
    /// The caller is responsible for actually starting a request with these
    /// callbacks, and storing the resulting request handle into `request`.
    fn begin_refresh(&mut self) -> ServerListCallbacks {
        self.servers.lock().unwrap().clear();
        *self.status.lock().unwrap() = QueryStatus::Refreshing;

        let responded_servers = self.servers.clone();
        let complete_servers = self.servers.clone();
        let complete_status = self.status.clone();

        ServerListCallbacks::new(
            // responded: one server's details are ready to fetch
            Box::new(move |request: Arc<Mutex<ServerListRequest>>, server_index: i32| {
                if let Ok(item) = request.lock().unwrap().get_server_details(server_index) {
                    responded_servers.lock().unwrap().push(item.into());
                }
            }),
            // failed: a server we asked about didn't respond in time
            Box::new(move |_request: Arc<Mutex<ServerListRequest>>, _server_index: i32| {}),
            // refresh_complete: the whole query is done
            Box::new(move |request: Arc<Mutex<ServerListRequest>>, response: ServerResponse| {
                let found = complete_servers.lock().unwrap().len();
                *complete_status.lock().unwrap() = QueryStatus::Done { found };
                if !matches!(response, ServerResponse::ServerResponded) {
                    warn!("Server list query finished with {response:?}");
                }
                // matches the pattern used by steamworks-rs's own tests:
                // release the request once we're done reading its results
                _ = request.lock().unwrap().release();
            }),
        )
    }
}

#[derive(Resource, Default)]
struct ServerBrowserState {
    internet: ServerQuery,
    lan: ServerQuery,
}

fn refresh_internet(steam: &SteamworksClient, app_id: u32, query: &mut ServerQuery) {
    if query.is_refreshing() {
        return;
    }
    let callbacks = query.begin_refresh();
    let request = steam
        .matchmaking_servers()
        .internet_server_list(app_id, &HashMap::new(), callbacks);
    query.request = request.ok();
}

fn refresh_lan(steam: &SteamworksClient, app_id: u32, query: &mut ServerQuery) {
    if query.is_refreshing() {
        return;
    }
    let callbacks = query.begin_refresh();
    let request = steam.matchmaking_servers().lan_server_list(app_id, callbacks);
    query.request = Some(request);
}

fn server_browser_ui(
    mut egui: EguiContexts,
    steam: Res<SteamworksClient>,
    config: Res<ServerBrowserConfig>,
    mut state: ResMut<ServerBrowserState>,
) -> Result<(), BevyError> {
    egui::Window::new("Server Browser").show(egui.ctx_mut()?, |ui| {
        ui.label(format!("App ID {}", config.app_id));

        ui.separator();
        ui.heading("Internet");
        if ui.button("Refresh").clicked() {
            refresh_internet(&steam, config.app_id, &mut state.internet);
        }
        server_list_ui(ui, "internet_server_browser_grid", &state.internet);

        ui.separator();
        ui.heading("LAN");
        if ui.button("Refresh").clicked() {
            refresh_lan(&steam, config.app_id, &mut state.lan);
        }
        server_list_ui(ui, "lan_server_browser_grid", &state.lan);
    });

    Ok(())
}

fn server_list_ui(ui: &mut egui::Ui, grid_id: &str, query: &ServerQuery) {
    let status = *query.status.lock().unwrap();
    ui.label(match status {
        QueryStatus::Idle => "Not queried yet - click Refresh".to_owned(),
        QueryStatus::Refreshing => "Refreshing...".to_owned(),
        QueryStatus::Done { found: 0 } => "No servers found".to_owned(),
        QueryStatus::Done { found } => format!("{found} server(s) found"),
    });

    let mut servers = query.servers.lock().unwrap();
    servers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    if servers.is_empty() {
        ui.weak(match status {
            QueryStatus::Idle => "Click Refresh to query the server list.",
            QueryStatus::Refreshing => "Waiting for servers to respond...",
            QueryStatus::Done { .. } => {
                "No servers responded. If you have a dedicated server running, check that it's \
                 using the same app ID, and that its query port is reachable."
            }
        });
    } else {
        egui::ScrollArea::vertical()
            .id_salt(grid_id)
            .max_height(200.0)
            .show(ui, |ui| {
                egui::Grid::new(grid_id).striped(true).show(ui, |ui| {
                    ui.strong("Name");
                    ui.strong("Map");
                    ui.strong("Players");
                    ui.strong("Ping");
                    ui.strong("Address");
                    ui.strong("Steam ID");
                    ui.end_row();

                    for server in servers.iter() {
                        ui.label(&server.name);
                        ui.label(&server.map);
                        ui.label(format!(
                            "{}/{} (+{} bots){}{}",
                            server.players,
                            server.max_players,
                            server.bots,
                            if server.has_password { " 🔒" } else { "" },
                            if server.secure { " 🛡" } else { "" },
                        ));
                        ui.label(format!("{}ms", server.ping_ms));
                        ui.label(server.addr.to_string());
                        ui.label(server.steam_id.to_string());
                        ui.end_row();
                    }
                });
            });
    }
}
