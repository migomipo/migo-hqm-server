use crate::hqm_game::HQMTeam;
use crate::hqm_server::{HQMServer, HQMServerPlayerData, HQMServerPlayerIndex};

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMDualControlSetting {
    No,
    Yes,
    Combined,
}

pub fn find_empty_dual_control(
    server: &HQMServer,
    team: HQMTeam,
) -> Option<(
    HQMServerPlayerIndex,
    Option<HQMServerPlayerIndex>,
    Option<HQMServerPlayerIndex>,
)> {
    for (i, player) in server.players.iter() {
        if let Some(player) = player {
            if let HQMServerPlayerData::DualControl { movement, stick } = player.data {
                if movement.is_none() || stick.is_none() {
                    if let Some((_, dual_control_team)) = player.object {
                        if dual_control_team == team {
                            return Some((i, movement, stick));
                        }
                    }
                }
            }
        }
    }
    None
}
