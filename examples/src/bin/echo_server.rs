//! Server which listens for client connections, and echoes back the UTF-8
//! strings that they send.
//!
//! This example shows you how to create a dedicated server, accept client
//! connections, and handle incoming messages. This example uses:
//! - `aeronet_steam` as the IO layer, using a Steam dedicated server under the
//!   hood. This is what actually receives and sends packets of `[u8]`s across
//!   the network.
//! - `aeronet_transport` as the transport layer, the default implementation.
//!   This manages reliability, ordering, and fragmentation of packets - meaning
//!   that all you have to worry about is the actual data payloads that you want
//!   to receive and send.
//!
//! This example is designed to work with the `echo_client` example.
//!
//! This example requires Steam to be running, and uses the Spacewar test app
//! ID (`480`), so it should work without owning any particular game. It only
//! works natively, since `aeronet_steam` does not support WASM.

// This is unfortunately required because of <https://github.com/rust-lang/cargo/issues/9208>
// You won't need this in your own code
cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        fn main() {
            panic!("not supported on WASM");
        }
    } else {

use {
    aeronet::{
        io::{
            Session,
            bytes::Bytes,
            connection::{Disconnected, DisconnectReason, LocalAddr},
            server::Server,
        },
        transport::{AeronetTransportPlugin, Transport, lane::LaneKind},
    },
    aeronet_steam::{
        SessionConfig, SteamworksClient, SteamworksServer, SteamworksSockets,
        dedicated_server::{
            ListenTarget, SessionRequest, SessionResponse, SteamNetDedicatedServer,
            SteamNetDedicatedServerPlugin,
        },
    },
    bevy::{log::LogPlugin, prelude::*},
    core::net::{Ipv4Addr, SocketAddr},
};

// Let's set up the app.

fn main() -> AppExit {
    // Set up a Steam dedicated server. Unlike a regular Steam client, this
    // logs on anonymously and doesn't require a Steam user to be signed in.
    let (server, server_callbacks) = steamworks::Server::init(
        Ipv4Addr::UNSPECIFIED,
        GAME_PORT,
        QUERY_PORT,
        steamworks::ServerMode::Authentication,
        env!("CARGO_PKG_VERSION"),
    )
    .expect("failed to initialize steam server");

    server.set_product("aeronet-echo");
    server.set_game_description("aeronet echo example");
    server.set_server_name("aeronet echo server");
    server.set_dedicated_server(true);
    server.log_on_anonymous();
    server.enable_heartbeats(true);

    server_callbacks
        .networking_utils()
        .init_relay_network_access();

    let socket_provider = SteamworksSockets::Server(SteamworksServer(server.clone()));

    App::new()
        .insert_resource(SteamworksServer(server))
        .insert_resource(SteamworksClient(server_callbacks))
        .insert_resource(socket_provider)
        // Steam callbacks must be pumped every frame for the IO layer to work.
        .add_systems(PreUpdate, |steam: Res<SteamworksClient>| {
            steam.run_callbacks();
        })
        .add_plugins((
            // Core Bevy plugins.
            LogPlugin::default(),
            MinimalPlugins,
            // We're using a Steam dedicated server, so we add this plugin.
            // This will automatically add `AeronetIoPlugin` as well, which sets
            // up the IO layer. However, it does *not* set up the transport
            // layer (since technically, you may want to swap it out and use
            // your own).
            SteamNetDedicatedServerPlugin,
            // Here we actually set up the transport layer.
            AeronetTransportPlugin,
        ))
        // Open the server on startup.
        .add_systems(Startup, setup)
        // Every frame, we receive messages and print them out.
        .add_systems(Update, echo_messages)
        // Set up some observers to run when the server or client state changes.
        .add_observer(on_opened)
        .add_observer(on_session_request)
        .add_observer(on_connected)
        .add_observer(on_disconnected)
        .run()
}

// Use fixed ports for this example, mapping to the address that the
// `echo_client` connects to.
const GAME_PORT: u16 = 25572;
const QUERY_PORT: u16 = 27016;

// Define what `aeronet_transport` lanes will be used on client connections.
// When using the transport layer, you must define in advance what lanes will be
// available.
// The receiving and sending lanes may be different, but in this example we will
// use the same lane configuration for both.
const LANES: [LaneKind; 1] = [LaneKind::ReliableOrdered];

fn setup(mut commands: Commands) {
    // Let's set up our Steam dedicated server socket, listening on a plain
    // socket address (rather than only accepting Steam peer-to-peer
    // connections).
    let target = ListenTarget::Addr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), GAME_PORT));

    // Spawn an entity to represent this server.
    let mut server = commands.spawn_empty();
    // Make an `EntityCommand` via `open`, which will set up and open this
    // server.
    server.queue(SteamNetDedicatedServer::open(
        SessionConfig::default(),
        target,
    ));
}

// Observe state change events using `Trigger`s
fn on_opened(trigger: On<Add, Server>, servers: Query<&LocalAddr>) {
    let server = trigger.event_target();
    let local_addr = servers
        .get(server)
        .expect("opened server should have a binding socket `LocalAddr`");
    info!("{server} opened on {}", **local_addr);
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

fn on_connected(trigger: On<Add, Session>, sessions: Query<&Session>, clients: Query<&ChildOf>, mut commands: Commands) {
    let client = trigger.event_target();
    let session = sessions
        .get(client)
        .expect("we are adding this component to this entity");
    // A `Connected` `Session` which is a `ChildOf` is a client of a server.
    let Ok(&ChildOf(server)) = clients.get(client) else {
        return;
    };
    info!("{client} connected to {server}");

    // Add `Transport` and configure it with our lanes so that we can send
    // and receive messages on this client.
    let transport = Transport::new(
        session,
        LANES,
        LANES,
        // Don't use `std::time::Instant::now`!
        // Instead, use `bevy::platform::time::Instant`.
        bevy::platform::time::Instant::now(),
    )
    .expect("packet MTU should be large enough to support transport");
    commands.entity(client).insert(transport);
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

// Receive messages and echo them back to the sender.
fn echo_messages(
    // Query..
    mut clients: Query<
        (
            Entity,         // ..the entity ID
            &mut Transport, // ..and the transport layer access
        ),
        (
            With<ChildOf>, /* ..for all sessions which are connected to one of our servers
                           (excludes local dedicated clients) */
        ),
    >,
) {
    for (client, mut transport) in &mut clients {
        // Explicitly deref the `Mut<Transport>` to get a `&mut Transport`
        // from which we can grab disjoint refs to `recv` and `send`.
        let transport = &mut *transport;

        for msg in transport.recv.msgs.drain() {
            let payload = msg.payload;

            // `payload` is a `Vec<u8>` - we have full ownership of the bytes received.
            // We'll turn it into a UTF-8 string, and resend it along the same
            // lane that we received it on.
            let text = String::from_utf8(payload).unwrap_or_else(|_| "(not UTF-8)".into());
            info!("{client} > {text}");

            let reply = format!("You sent: {text}");
            info!("{client} < {reply}");
            // Convert our `String` into a `Bytes` to send it out.
            // We ignore the resulting `MessageKey`, since we don't need it.
            _ = transport.send.push(
                msg.lane,
                Bytes::from(reply),
                bevy::platform::time::Instant::now(),
            );
        }

        for _ in transport.recv.acks.drain() {
            // We have to use up acknowledgements,
            // but since we don't actually care about reading them,
            // we'll just ignore them.
        }
    }
}

}}
