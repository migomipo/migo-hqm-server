use crate::hqm_server::{HQMInitialGameValues, HQMServer, HQMServerPlayerIndex};
use crate::hqm_simulate::HQMSimulationEvent;

pub trait HQMServerBehaviour {
    fn init(&mut self, _server: &mut HQMServer) {}

    fn before_tick(&mut self, server: &mut HQMServer);
    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]);
    fn handle_command(
        &mut self,
        _server: &mut HQMServer,
        _cmd: &str,
        _arg: &str,
        _player_index: HQMServerPlayerIndex,
    ) {
    }

    fn get_initial_game_values(&mut self) -> HQMInitialGameValues;
    fn game_started(&mut self, _server: &mut HQMServer) {}

    fn before_player_exit(&mut self, _server: &mut HQMServer, _player_index: HQMServerPlayerIndex) {
    }

    fn after_player_join(&mut self, _server: &mut HQMServer, _player_index: HQMServerPlayerIndex) {}

    fn get_number_of_players(&self) -> u32;

    fn save_replay_data(&self, _server: &HQMServer) -> bool {
        false
    }
}
