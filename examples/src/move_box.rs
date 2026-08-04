//! Demo app where clients can connect to a server and control a box with the
//! arrow keys.
//!
//! Box positions are synced between clients and servers using [`bevy_replicon`]
//! with the [`aeronet_replicon`] backend.
//!
//! This example uses [`aeronet_steam`] as its IO layer, with the server
//! hosting a Steam dedicated server (via [`aeronet_steam::dedicated_server`]).
//!
//! Based on <https://github.com/projectharmonia/bevy_replicon_renet/blob/master/examples/simple_box.rs>.
//!
//! # Usage
//!
//! This example requires Steam to be running, and uses the Spacewar test
//! app ID (`480`), so it should work without owning any particular game.
//!
//! ## Server
//!
//! ```sh
//! cargo run --bin move_box_server
//! ```
//!
//! ## Client
//!
//! ```sh
//! cargo run --bin move_box_client
//! ```
//!
//! Steam dedicated servers don't run under WASM, so this example is
//! native-only.
//!
//! [`aeronet_steam`]: https://docs.rs/aeronet_steam
//! [`bevy_replicon`]: https://docs.rs/bevy_replicon
//! [`aeronet_replicon`]: https://docs.rs/aeronet_replicon

use {
    bevy::prelude::*,
    bevy_replicon::prelude::*,
    serde::{Deserialize, Serialize},
};

/// Steam app ID used to initialize Steamworks.
///
/// This is the Spacewar test app, freely usable for testing Steamworks
/// integrations.
pub const STEAM_APP_ID: u32 = 480;

/// Port registered with `ISteamGameServer` for game info, used by the Steam
/// master server / server browser to identify this server.
///
/// This is *not* the port that clients actually connect to for gameplay
/// traffic - see [`STEAM_NET_PORT`] for that.
pub const STEAM_GAME_PORT: u16 = 25572;

/// Port that the Steam dedicated server uses for master server queries.
pub const STEAM_QUERY_PORT: u16 = 27016;

/// Port that the [`aeronet_steam`] game socket (`SteamNetDedicatedServer`)
/// actually listens on for client connections.
///
/// This must be different from [`STEAM_GAME_PORT`]/[`STEAM_QUERY_PORT`],
/// since those are bound internally by `ISteamGameServer` for master server
/// registration - reusing the same port number for both causes them to
/// fight over the same OS socket, breaking the master server's ability to
/// verify this server is alive (and therefore breaking Internet server
/// browser visibility, even though direct/LAN connections still work).
///
/// [`aeronet_steam`]: https://docs.rs/aeronet_steam
pub const STEAM_NET_PORT: u16 = 27015;

/// How many units a player may move in a single second.
const MOVE_SPEED: f32 = 250.0;

/// How many times per second we will replicate entity components.
pub const TICK_RATE: u16 = 128;

/// Sets up replication and basic game systems.
pub struct MoveBoxPlugin;

/// Whether the game is currently being simulated or not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, States)]
pub enum GameState {
    /// Game is not being simulated.
    #[default]
    None,
    /// Game is being simulated.
    Playing,
}

impl Plugin for MoveBoxPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .replicate::<Player>()
            .replicate::<PlayerPosition>()
            .replicate::<PlayerColor>()
            .add_client_message::<PlayerInput>(Channel::Unreliable)
            .add_systems(
                FixedUpdate,
                (recv_input, apply_movement)
                    .chain()
                    .run_if(in_state(ClientState::Disconnected)),
            );
    }
}

/// Marker component for a player in the game.
#[derive(Debug, Clone, Component, Serialize, Deserialize)]
#[require(DespawnOnExit::<GameState>(GameState::Playing))]
pub struct Player;

/// Player's box position.
#[derive(Debug, Clone, Component, Deref, DerefMut, Serialize, Deserialize)]
pub struct PlayerPosition(pub Vec2);

/// Player's box color.
#[derive(Debug, Clone, Component, Deref, DerefMut, Serialize, Deserialize)]
pub struct PlayerColor(pub Color);

/// Player's inputs that they send to control their box.
#[derive(Debug, Clone, Default, Component, Message, Serialize, Deserialize)]
pub struct PlayerInput {
    /// Lateral movement vector.
    ///
    /// The client has full control over this field, and may send an
    /// unnormalized vector! Authorities must ensure that they normalize or
    /// zero this vector before using it for movement updates.
    pub movement: Vec2,
}

fn recv_input(
    mut inputs: MessageReader<FromClient<PlayerInput>>,
    mut players: Query<&mut PlayerInput>,
) {
    for &FromClient {
        client_id,
        message: ref new_input,
    } in inputs.read()
    {
        let ClientId::Client(client_entity) = client_id else {
            continue;
        };

        let Ok(mut input) = players.get_mut(client_entity) else {
            continue;
        };
        *input = new_input.clone();
    }
}

fn apply_movement(time: Res<Time>, mut players: Query<(&PlayerInput, &mut PlayerPosition)>) {
    for (input, mut position) in &mut players {
        // make sure to validate inputs and normalize on the authority (server) side,
        // since we're accepting arbitrary client input
        if let Some(movement) = input.movement.try_normalize() {
            // only change `position` if we actually have a movement vector to apply
            // this saves bandwidth; we don't replicate position if we don't change it
            **position += movement * time.delta_secs() * MOVE_SPEED;
        }
    }
}
