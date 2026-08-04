//! See `src/move_box.rs`.

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        fn main() {
            panic!("not supported on WASM");
        }
    } else {

use {
    aeronet::{
        io::{
            Session, SessionEndpoint,
            connection::{Disconnect, DisconnectReason, Disconnected},
        },
        transport::{
            TransportConfig,
            sampling::SessionStats,
            visualizer::{SessionVisualizer, SessionVisualizerPlugin},
        },
    },
    aeronet_replicon::client::{AeronetRepliconClient, AeronetRepliconClientPlugin},
    aeronet_steam::{
        SessionConfig, SteamworksClient, SteamworksSockets,
        client::{ConnectTarget, SteamNetClient, SteamNetClientPlugin},
    },
    bevy::{log::LogPlugin, prelude::*},
    bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui},
    bevy_replicon::prelude::*,
    core::net::SocketAddr,
    examples::move_box::{
        GameState, MoveBoxPlugin, PlayerColor, PlayerInput, PlayerPosition, STEAM_APP_ID,
        STEAM_NET_PORT,
    },
    steamworks::SteamId,
};

fn main() -> AppExit {
    let steam = steamworks::Client::init_app(STEAM_APP_ID).expect("failed to initialize steam");
    steam.networking_utils().init_relay_network_access();

    let socket_provider = SteamworksSockets::Client(SteamworksClient(steam.clone()));

    App::new()
        .insert_resource(SteamworksClient(steam))
        .insert_resource(socket_provider)
        .add_systems(PreUpdate, |steam: Res<SteamworksClient>| {
            steam.run_callbacks();
        })
        .add_plugins((
            // core
            DefaultPlugins.set(LogPlugin {
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
            }),
            EguiPlugin::default(),
            // transport
            SteamNetClientPlugin,
            SessionVisualizerPlugin,
            // replication
            RepliconPlugins,
            AeronetRepliconClientPlugin,
            // game
            MoveBoxPlugin,
        ))
        .init_resource::<GlobalUi>()
        .init_resource::<SteamUi>()
        .add_systems(Startup, setup_ui)
        .add_systems(
            Update,
            (draw_boxes, handle_inputs, log_session_stats).run_if(in_state(GameState::Playing)),
        )
        .add_systems(EguiPrimaryContextPass, (steam_ui, global_ui).chain())
        .add_observer(on_connecting)
        .add_observer(on_connected)
        .add_observer(on_disconnected)
        .run()
}

#[derive(Debug, Default, Resource)]
struct GlobalUi {
    session_id: usize,
    log: Vec<String>,
}

#[derive(Debug, Default, Resource)]
struct SteamUi {
    target_addr: String,
    target_peer: String,
}

fn setup_ui(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn on_connecting(
    trigger: On<Add, SessionEndpoint>,
    names: Query<&Name>,
    mut ui_state: ResMut<GlobalUi>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let name = names
        .get(entity)
        .expect("our session entity should have a name");
    ui_state.log.push(format!("{name} connecting"));

    // IMPORTANT
    //
    // Make sure to insert this component into your client entity,
    // so that `aeronet_replicon` knows you want to use this for `bevy_replicon`!
    //
    // You can also do this when `spawn`ing the entity instead, which is a bit more
    // efficient. We just do it on `On<Add, SessionEndpoint>`, since we have
    // multiple `spawn` calls, and it's nicer to centralize inserting this
    // component in a single place.
    commands.entity(entity).insert(AeronetRepliconClient);
}

fn on_connected(
    trigger: On<Add, Session>,
    names: Query<&Name>,
    mut ui_state: ResMut<GlobalUi>,
    mut game_state: ResMut<NextState<GameState>>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let name = names
        .get(entity)
        .expect("our session entity should have a name");
    ui_state.log.push(format!("{name} connected"));

    game_state.set(GameState::Playing);
    commands.entity(entity).insert((
        SessionVisualizer::default(),
        TransportConfig {
            max_memory_usage: 64 * 1024,
            tx_bytes_per_sec: 4 * 1024,
            ..default()
        },
    ));
}

fn on_disconnected(
    trigger: On<Disconnected>,
    names: Query<&Name>,
    mut ui_state: ResMut<GlobalUi>,
    mut game_state: ResMut<NextState<GameState>>,
) {
    let session = trigger.event_target();
    let name = names
        .get(session)
        .expect("our session entity should have a name");
    let message = match &trigger.reason {
        DisconnectReason::ByUser(reason) => {
            format!("{name} disconnected by user: {reason}")
        }
        DisconnectReason::ByPeer(reason) => {
            format!("{name} disconnected by peer: {reason}")
        }
        DisconnectReason::ByError(err) => {
            format!("{name} disconnected due to error: {err:#}")
        }
    };
    // Also log to the console/log file, not just the egui window - this is
    // the reason the disconnect actually happened, and it's easy to miss in
    // the UI if the app closes or you're not watching it at the time.
    warn!("{message}");
    ui_state.log.push(message);
    game_state.set(GameState::None);
}

fn global_ui(
    mut commands: Commands,
    mut egui: EguiContexts,
    global_ui: Res<GlobalUi>,
    sessions: Query<(Entity, &Name, Option<&Session>), With<SessionEndpoint>>,
    client_state: Res<State<ClientState>>,
    client_stats: Res<ClientStats>,
) -> Result<(), BevyError> {
    egui::Window::new("Session Log").show(egui.ctx_mut()?, |ui| {
        ui.label("Replicon reports:");
        ui.horizontal(|ui| {
            ui.label(match client_state.get() {
                ClientState::Disconnected => "Disconnected",
                ClientState::Connecting => "Connecting",
                ClientState::Connected => "Connected",
            });
            ui.separator();

            ui.label(format!("RTT {:.0}ms", client_stats.rtt * 1000.0));
            ui.separator();

            ui.label(format!("Pkt Loss {:.1}%", client_stats.packet_loss * 100.0));
            ui.separator();

            ui.label(format!("Rx {:.0}bps", client_stats.received_bps));
            ui.separator();

            ui.label(format!("Tx {:.0}bps", client_stats.sent_bps));
        });
        match sessions.single() {
            Ok((session, name, connected)) => {
                if connected.is_some() {
                    ui.label(format!("{name} connected"));
                } else {
                    ui.label(format!("{name} connecting"));
                }

                if ui.button("Disconnect").clicked() {
                    commands.trigger(Disconnect::new(session, "pressed disconnect button"));
                }
            }
            Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => {
                ui.label("No sessions active");
            }
            Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
                ui.label("Multiple sessions active");
            }
        }

        ui.separator();

        for msg in &global_ui.log {
            ui.label(msg);
        }
    });

    Ok(())
}

//
// Steam
//

fn steam_ui(
    mut commands: Commands,
    mut egui: EguiContexts,
    mut global_ui: ResMut<GlobalUi>,
    mut ui_state: ResMut<SteamUi>,
    sessions: Query<(), With<Session>>,
) -> Result<(), BevyError> {
    let default_target = format!("127.0.0.1:{STEAM_NET_PORT}");

    egui::Window::new("Steam").show(egui.ctx_mut()?, |ui| {
        if sessions.iter().next().is_some() {
            ui.disable();
        }

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));

        let mut connect_addr = false;
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut ui_state.target_addr)
                    .hint_text(format!("{default_target} | [enter] to connect")),
            );
            connect_addr |= resp.lost_focus() && enter_pressed;
            connect_addr |= ui.button("Connect to address").clicked();
        });

        let mut connect_peer = false;
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut ui_state.target_peer)
                    .hint_text("Steam ID | [enter] to connect"),
            );
            connect_peer |= resp.lost_focus() && enter_pressed;
            connect_peer |= ui.button("Connect to Steam ID").clicked();
        });

        if connect_addr {
            let mut target = ui_state.target_addr.clone();
            if target.is_empty() {
                target = default_target;
            }

            match target.parse::<SocketAddr>() {
                Ok(target) => {
                    global_ui.session_id += 1;
                    let name = format!("{}. {target}", global_ui.session_id);
                    commands
                        .spawn(Name::new(name))
                        .queue(SteamNetClient::connect(SessionConfig::default(), target));
                }
                Err(err) => {
                    global_ui.log.push(format!("Invalid address `{target}`: {err:?}"));
                }
            }
        }

        if connect_peer {
            let target = ui_state.target_peer.clone();

            match target.parse::<u64>() {
                Ok(target) => {
                    let target = SteamId::from_raw(target);
                    global_ui.session_id += 1;
                    let name = format!("{}. {target:?}", global_ui.session_id);
                    commands.spawn(Name::new(name)).queue(SteamNetClient::connect(
                        SessionConfig::default(),
                        ConnectTarget::from(target),
                    ));
                }
                Err(err) => {
                    global_ui
                        .log
                        .push(format!("Invalid Steam ID `{target}`: {err:?}"));
                }
            }
        }
    });

    Ok(())
}

//
// game logic
//

fn handle_inputs(mut inputs: MessageWriter<PlayerInput>, input: Res<ButtonInput<KeyCode>>) {
    let mut movement = Vec2::ZERO;
    if input.pressed(KeyCode::ArrowRight) {
        movement.x += 1.0;
    }
    if input.pressed(KeyCode::ArrowLeft) {
        movement.x -= 1.0;
    }
    if input.pressed(KeyCode::ArrowUp) {
        movement.y += 1.0;
    }
    if input.pressed(KeyCode::ArrowDown) {
        movement.y -= 1.0;
    }

    // don't normalize here, since the server will normalize anyway
    inputs.write(PlayerInput { movement });
}

fn draw_boxes(mut gizmos: Gizmos, players: Query<(&PlayerPosition, &PlayerColor)>) {
    for (PlayerPosition(pos), PlayerColor(color)) in &players {
        gizmos.rect_2d(*pos, Vec2::ONE * 50.0, *color);
    }
}

//
// diagnostics
//

/// Periodically logs RTT/packet-loss/throughput for the active session, so
/// you can see the connection quality trending downward (or an abrupt
/// silence) in the seconds leading up to an unexpected disconnect - rather
/// than only finding out after the fact from the disconnect reason alone.
fn log_session_stats(
    time: Res<Time>,
    mut since_last_log: Local<f32>,
    sessions: Query<(&Name, &SessionStats), With<Session>>,
) {
    const LOG_INTERVAL_SECS: f32 = 1.0;

    *since_last_log += time.delta_secs();
    if *since_last_log < LOG_INTERVAL_SECS {
        return;
    }
    *since_last_log = 0.0;

    for (name, stats) in &sessions {
        let Some(sample) = stats.last() else {
            continue;
        };
        info!(
            "{name}: RTT {:.0}ms, loss {:.1}%, sent {} / recv {} packets ({} / {} bytes)",
            sample.msg_rtt.as_secs_f64() * 1000.0,
            sample.loss * 100.0,
            sample.packets_delta.packets_sent,
            sample.packets_delta.packets_recv,
            sample.packets_delta.bytes_sent,
            sample.packets_delta.bytes_recv,
        );
    }
}

}}
