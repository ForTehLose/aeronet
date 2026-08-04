//! Client which connects to an echo server, and sends/receives plain UTF-8
//! strings.
//!
//! This example shows you how to create a client, establish a connection to a
//! server, and send and receive messages. This example uses:
//! - `aeronet_steam` as the IO layer, using Steam networking sockets under the
//!   hood. This is what actually sends packets of `[u8]`s across the network.
//! - `aeronet_transport` as the transport layer, the default implementation.
//!   This manages reliability, ordering, and fragmentation of packets - meaning
//!   that all you have to worry about is the actual data payloads that you want
//!   to send.
//!
//! This example requires Steam to be running, and uses the Spacewar test app
//! ID (`480`), so it should work without owning any particular game. It only
//! works natively, since `aeronet_steam` does not support WASM.

use {
    aeronet::{
        io::{
            Session, SessionEndpoint,
            bytes::Bytes,
            connection::{Disconnect, DisconnectReason, Disconnected},
        },
        transport::{
            AeronetTransportPlugin, Transport, TransportConfig,
            lane::{LaneIndex, LaneKind},
        },
    },
    aeronet_steam::{
        SessionConfig, SteamworksClient, SteamworksSockets,
        client::{SteamNetClient, SteamNetClientPlugin},
    },
    bevy::prelude::*,
    bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui},
    core::{mem, net::SocketAddr},
};

// Let's set up the app.

fn main() -> AppExit {
    // Steam must be running, and we identify ourselves with an app ID.
    // `480` is Valve's public Spacewar test app, usable for testing without
    // owning a specific game on Steam.
    let steam = steamworks::Client::init_app(480).expect("failed to initialize steam");
    steam.networking_utils().init_relay_network_access();

    let socket_provider = SteamworksSockets::Client(SteamworksClient(steam.clone()));

    App::new()
        .insert_resource(SteamworksClient(steam))
        .insert_resource(socket_provider)
        // Steam callbacks must be pumped every frame for the IO layer to work.
        .add_systems(PreUpdate, |steam: Res<SteamworksClient>| {
            steam.run_callbacks();
        })
        .add_plugins((
            DefaultPlugins,
            // We'll use `bevy_egui` for displaying the UI.
            EguiPlugin::default(),
            // We're using Steam networking sockets, so we add this plugin.
            // This will automatically add `AeronetIoPlugin` as well, which sets
            // up the IO layer. However, it does *not* set up the transport
            // layer (since technically, you may want to swap it out and use
            // your own).
            SteamNetClientPlugin,
            // Here we actually set up the transport layer.
            AeronetTransportPlugin,
        ))
        // Connect to the server on startup.
        .add_systems(Startup, (setup_ui, setup_connection))
        // Every frame, we..
        .add_systems(Update, recv_messages) // ..receive messages and push them into the session's `UiState`
        .add_systems(EguiPrimaryContextPass, ui) // ..draw the UI for the session
        // Set up some observers to run when the session state changes
        .add_observer(on_connecting)
        .add_observer(on_connected)
        .add_observer(on_disconnected)
        .run()
}

#[derive(Debug, Default, Component)]
struct UiState {
    msg: String,
    log: Vec<String>,
}

// Default address that we'll be connecting to.
const DEFAULT_TARGET: &str = "127.0.0.1:25572";

// Define what `aeronet_transport` lanes will be used on this connection.
// When using the transport layer, you must define in advance what lanes will be
// available.
// The receiving and sending lanes may be different, but in this example we will
// use the same lane configuration for both.
const LANES: [LaneKind; 1] = [LaneKind::ReliableOrdered];

// When sending out messages, we have to specify what lane we're sending out on.
// This determines the delivery guarantees e.g. reliability and ordering.
// Since we configured only 1 lane (index 0), we'll send on that lane.
const SEND_LANE: LaneIndex = LaneIndex::new(0);

fn setup_ui(mut commands: Commands) {
    // Required for `bevy_egui` to render content.
    // Otherwise you'll just get a blank window.
    commands.spawn(Camera2d);
}

fn setup_connection(mut commands: Commands) {
    // Let's start a connection to a Steam networking sockets server.

    // First, make the configuration.
    let config = SessionConfig::default();
    // And define what address we want to connect to.
    let target: SocketAddr = DEFAULT_TARGET.parse().expect("should be a valid address");

    // Spawn an entity to represent this session.
    let mut entity = commands.spawn((
        // Add the `TransportConfig` to configure some settings for the
        // `aeronet_transport::Transport` we'll add later.
        // We can't add that component just yet, since we don't have a
        // `Session`, but we will later.
        // This component is optional - if `Transport` is added without it,
        // a default `TransportConfig` will also be added.
        TransportConfig {
            // Define how many bytes of memory this session can use
            // for transport state.
            max_memory_usage: 4 * 1024 * 1024,
            ..default()
        },
        // Add `UiState` so that we can log what messages we've received.
        UiState::default(),
    ));
    // Make an `EntityCommand` via `connect`, which will set up this
    // session, and push that command onto the entity.
    entity.queue(SteamNetClient::connect(config, target));
}

// Observe state change events using `Trigger`s.
fn on_connecting(trigger: On<Add, SessionEndpoint>, mut sessions: Query<&mut UiState>) {
    let entity = trigger.event_target();
    let mut ui_state = sessions
        .get_mut(entity)
        .expect("our sessions should have these components");
    ui_state.log.push(format!("{entity} connecting"));
}

fn on_connected(
    trigger: On<Add, Session>,
    mut sessions: Query<(&Session, &mut UiState)>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let (session, mut ui_state) = sessions
        .get_mut(entity)
        .expect("our sessions should have these components");
    ui_state.log.push(format!("{entity} connected"));

    // Once the `Session` is added, we can make a `Transport`
    // and use messages.
    let transport = Transport::new(
        session,
        LANES,
        LANES,
        // Don't use `std::time::Instant::now`!
        // Instead, use `bevy::platform::time::Instant`.
        bevy::platform::time::Instant::now(),
    )
    .expect("packet MTU should be large enough to support transport");
    commands.entity(entity).insert(transport);
}

fn on_disconnected(trigger: On<Disconnected>) {
    let entity = trigger.event_target();
    match &trigger.reason {
        DisconnectReason::ByUser(reason) => info!("{entity} disconnected by user: {reason}"),
        DisconnectReason::ByPeer(reason) => info!("{entity} disconnected by peer: {reason}"),
        DisconnectReason::ByError(err) => warn!("{entity} disconnected due to error: {err:#}"),
    }
}

// Receive messages and add them to the log.
fn recv_messages(
    // Query..
    mut sessions: Query<
        (
            &mut Transport, // ..the messages received by the transport layer
            &mut UiState,   // ..and push the messages into `UiState::log`
        ),
        Without<ChildOf>, /* ..for all sessions which aren't parented to a server (so only our
                           * own local clients) */
    >,
) {
    for (mut transport, mut ui_state) in &mut sessions {
        for msg in transport.recv.msgs.drain() {
            let payload = msg.payload;

            // `payload` is a `Vec<u8>` - we have full ownership of the bytes received.
            // We'll turn it into a UTF-8 string.
            // We don't care about the lane index.
            let text = String::from_utf8(payload).unwrap_or_else(|_| "(not UTF-8)".into());
            ui_state.log.push(format!("> {text}"));
        }

        for _ in transport.recv.acks.drain() {
            // We have to use up acknowledgements,
            // but since we don't actually care about reading them,
            // we'll just ignore them.
        }
    }
}

fn ui(
    mut egui: EguiContexts,
    // We'll use `Commands` to trigger `Disconnect`s
    // if the user presses the disconnect button.
    mut commands: Commands,
    // Technically, this query can run for multiple sessions, so we can have
    // multiple `egui` windows. But there will only ever be 1 session active.
    mut sessions: Query<(Entity, &mut Transport, &mut UiState), Without<ChildOf>>,
) -> Result<(), BevyError> {
    for (entity, mut transport, mut ui_state) in &mut sessions {
        egui::Window::new("Log").show(egui.ctx_mut()?, |ui| {
            ui.text_edit_singleline(&mut ui_state.msg);

            if ui.button("Send").clicked() {
                // Send the message out.
                let msg = mem::take(&mut ui_state.msg);
                ui_state.log.push(format!("< {msg}"));

                let msg = Bytes::from(msg);
                // We ignore the resulting `MessageKey`, since we don't need it.
                _ = transport
                    .send
                    .push(SEND_LANE, msg, bevy::platform::time::Instant::now());
            }

            if ui.button("Disconnect").clicked() {
                // Here's how you disconnect the session with a given reason.
                // Don't just remove components or despawn entities - use `Disconnect` instead!
                commands.trigger(Disconnect::new(entity, "pressed disconnect button"));
            }

            for line in &ui_state.log {
                ui.label(line);
            }
        });
    }

    Ok(())
}
