//! See `src/move_box.rs`.

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        fn main() {
            panic!("not supported on WASM");
        }
    } else {

use {
    aeronet::io::{
        Session,
        connection::{Disconnected, DisconnectReason, LocalAddr},
        server::Server,
    },
    aeronet_replicon::server::{AeronetRepliconServer, AeronetRepliconServerPlugin},
    aeronet_steam::{
        SessionConfig, SteamworksClient, SteamworksServer, SteamworksSockets,
        dedicated_server::{
            ListenTarget, SessionRequest, SessionResponse, SteamNetDedicatedServer,
            SteamNetDedicatedServerPlugin,
        },
    },
    bevy::{
        app::ScheduleRunnerPlugin, ecs::schedule::ScheduleLabel, log::LogPlugin, prelude::*,
        state::app::StatesPlugin,
    },
    bevy_replicon::prelude::*,
    core::{
        net::{Ipv4Addr, SocketAddr},
        time::Duration,
    },
    examples::move_box::{
        MoveBoxPlugin, Player, PlayerColor, PlayerInput, PlayerPosition, STEAM_GAME_PORT,
        STEAM_NET_PORT, STEAM_QUERY_PORT, TICK_RATE,
    },
    std::time::SystemTime,
};

/// `move_box` demo server
#[derive(Debug, Resource, clap::Parser)]
struct Args {
    /// Port to listen for Steam game connections on
    #[arg(long, default_value_t = STEAM_GAME_PORT)]
    game_port: u16,
    /// Port used for Steam master server queries
    #[arg(long, default_value_t = STEAM_QUERY_PORT)]
    query_port: u16,
    /// Port that the actual game socket listens on for client connections
    ///
    /// Must be different from `game_port`/`query_port` - see
    /// [`STEAM_NET_PORT`] for why.
    #[arg(long, default_value_t = STEAM_NET_PORT)]
    net_port: u16,
}

fn main() -> AppExit {
    let args = <Args as clap::Parser>::parse();

    let (server, server_callbacks) = steamworks::Server::init(
            Ipv4Addr::LOCALHOST,
            25572,
            27016,
            steamworks::ServerMode::AuthenticationAndSecure,
            "1.0.0.0",
    )
    .expect("failed to initialize steam server");

    // server.set_product("480");
    // server.set_game_description("spacewar");
    // server.set_map_name("move_box");
    // server.set_max_players(16);
    // server.set_server_name("aeronet move_box server");
    server.set_game_description("Description");
        server.set_mod_dir("spacewar");
        server.set_product("spacewar");
        server.set_map_name("island");
        server.set_max_players(16);
        server.set_server_name("Some Server");
    server.set_dedicated_server(true);
    server.log_on_anonymous();
    server.enable_heartbeats(true);

    server_callbacks
        .networking_utils()
        .init_relay_network_access();

    // The game server's log-on to Steam (needed for the server browser
    // heartbeat to work) happens asynchronously. Register callbacks so we get
    // clear, unmissable feedback on whether it actually succeeded - if you
    // never see "connected to Steam", the server will never appear in the
    // browser regardless of anything else in this file.
    // We deliberately leak the handles: they just need to live for the
    // program's lifetime.
    Box::leak(Box::new(server_callbacks.register_callback(
        |event: steamworks::SteamServersConnected| {
            info!("Game server connected to Steam: {event:?}");
        },
    )));
    Box::leak(Box::new(server_callbacks.register_callback(
        |event: steamworks::SteamServerConnectFailure| {
            warn!("Game server FAILED to connect to Steam: {event:?}");
        },
    )));
    Box::leak(Box::new(server_callbacks.register_callback(
        |event: steamworks::SteamServersDisconnected| {
            warn!("Game server disconnected from Steam: {event:?}");
        },
    )));

    let steam_id = server.steam_id();
    info!("Steam server ID: {steam_id:?}");

    let socket_provider = SteamworksSockets::Server(SteamworksServer(server.clone()));

    App::new()
        .insert_resource(args)
        .insert_resource(SteamworksServer(server))
        .insert_resource(SteamworksClient(server_callbacks))
        .insert_resource(socket_provider)
        .add_systems(PreUpdate, |steam: Res<SteamworksClient>| {
            steam.run_callbacks();
        })
        .add_plugins((
            // core
            LogPlugin {
                // Surface `aeronet_steam`/`aeronet_transport`'s internal
                // `debug!`/`trace!` logging (connection state changes, real
                // Steam-level disconnect reasons, packet send/recv counts)
                // which is silent at the default `info` level. Useful when
                // diagnosing unexpected disconnects.
                filter: format!(
                    "{},aeronet_steam=debug,aeronet_transport=debug",
                    bevy::log::DEFAULT_FILTER
                ),
                ..default()
            },
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                1.0 / f64::from(TICK_RATE),
            ))),
            StatesPlugin,
            // transport
            SteamNetDedicatedServerPlugin,
            // replication
            RepliconPlugins.set(ServerPlugin {
                // 1 frame lasts `1.0 / TICK_RATE` anyway
                tick_schedule: PostUpdate.intern(),
                ..Default::default()
            }),
            AeronetRepliconServerPlugin,
            // game
            MoveBoxPlugin,
        ))
        .add_systems(Startup, open_server)
        .add_observer(on_opened)
        .add_observer(on_session_request)
        .add_observer(on_connected)
        .add_observer(on_disconnected)
        .run()
}

fn open_server(mut commands: Commands, args: Res<Args>) {
    // let target = ListenTarget::Addr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), args.net_port));
    let target = ListenTarget::Peer { virtual_port: 0 };

    let server = commands
        .spawn((
            Name::new("Steam Dedicated Server"),
            // IMPORTANT
            //
            // Make sure to insert this component into your server entity,
            // so that `aeronet_replicon` knows you want to use this for `bevy_replicon`!
            AeronetRepliconServer,
        ))
        .queue(SteamNetDedicatedServer::open(
            SessionConfig::default(),
            target,
        ))
        .id();
    info!("Opening Steam dedicated server {server}");
}

fn on_opened(trigger: On<Add, Server>, servers: Query<&LocalAddr>) {
    let server = trigger.event_target();
    if let Ok(local_addr) = servers.get(server) {
        info!("{server} opened on {:?}", **local_addr);
    } else {
        info!("{server} opened for peer connections");
    }
}

fn on_session_request(mut request: On<SessionRequest>, clients: Query<&ChildOf>) {
    let client = request.event_target();
    let Ok(&ChildOf(server)) = clients.get(client) else {
        return;
    };

    info!(
        "{client} connecting to {server} with Steam ID {:?}",
        request.steam_id
    );
    request.respond(SessionResponse::Accepted);
}

fn on_connected(trigger: On<Add, Session>, clients: Query<&ChildOf>, mut commands: Commands) {
    let client = trigger.event_target();
    let Ok(&ChildOf(server)) = clients.get(client) else {
        return;
    };
    info!("{client} connected to {server}");

    // generate a random-looking color
    let time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("current system time should be after unix epoch")
        .as_millis();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "truncation is what we want"
    )]
    let color = Color::srgb_u8((time * 3) as u8, (time * 5) as u8, (time * 7) as u8);

    commands.entity(client).insert((
        Player,
        PlayerPosition(Vec2::ZERO),
        PlayerColor(color),
        PlayerInput::default(),
        Replicated,
    ));
}

fn on_disconnected(trigger: On<Disconnected>, clients: Query<&ChildOf>) {
    let client = trigger.event_target();
    let Ok(&ChildOf(server)) = clients.get(client) else {
        return;
    };

    match &trigger.reason {
        DisconnectReason::ByUser(reason) => {
            info!("{client} disconnected from {server} by user: {reason}");
        }
        DisconnectReason::ByPeer(reason) => {
            info!("{client} disconnected from {server} by peer: {reason}");
        }
        DisconnectReason::ByError(err) => {
            warn!("{client} disconnected from {server} due to error: {err:#}");
        }
    }
}

}}
