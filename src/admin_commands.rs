use crate::server::{HQMServer, MuteStatus, PlayerListExt, ServerPlayerData};

use crate::game::PlayerIndex;
use crate::gamemode::{ExitReason, GameMode};
use crate::ReplayEnabled;
use systemctl::restart;
use tracing::info;

impl HQMServer {
    pub(crate) fn set_allow_join(&mut self, player_index: PlayerIndex, allowed: bool) {
        if let Some(player) = self.state.players.get_player(player_index) {
            if player.is_admin {
                self.allow_join = allowed;

                if allowed {
                    info!("{} ({}) enabled joins", player.player_name, player_index);
                    let msg = format!("Joins enabled by {}", player.player_name);
                    self.state.add_server_chat_message(msg);
                } else {
                    info!("{} ({}) disabled joins", player.player_name, player_index);
                    let msg = format!("Joins disabled by {}", player.player_name);
                    self.state.add_server_chat_message(msg);
                }
            } else {
                self.state.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn mute_player(
        &mut self,
        admin_player_index: PlayerIndex,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self.state.players.get_player(admin_player_index) {
            if admin_player.is_admin {
                let admin_player_name = admin_player.player_name.clone();

                if let Some(mute_player) = self.state.players.get_player_mut(mute_player_index) {
                    mute_player.is_muted = MuteStatus::Muted;
                    info!(
                        "{} ({}) muted {} ({})",
                        admin_player_name,
                        admin_player_index,
                        mute_player.player_name,
                        mute_player_index
                    );
                    let msg = format!("{} muted by {}", mute_player.player_name, admin_player_name);
                    self.state.add_server_chat_message(msg);
                }
            } else {
                self.state.admin_deny_message(admin_player_index);
            }
        }
    }

    pub(crate) fn unmute_player(
        &mut self,
        admin_player_index: PlayerIndex,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self.state.players.get_player(admin_player_index) {
            if admin_player.is_admin {
                let admin_player_name = admin_player.player_name.clone();

                if let Some(mute_player) = self.state.players.get_player_mut(mute_player_index) {
                    let old_status = mute_player.is_muted;
                    mute_player.is_muted = MuteStatus::NotMuted;
                    info!(
                        "{} ({}) unmuted {} ({})",
                        admin_player_name,
                        admin_player_index,
                        mute_player.player_name,
                        mute_player_index
                    );
                    let msg = format!(
                        "{} unmuted by {}",
                        mute_player.player_name, admin_player_name
                    );
                    if old_status == MuteStatus::Muted {
                        self.state.add_server_chat_message(msg);
                    } else {
                        self.state
                            .add_directed_server_chat_message(msg, admin_player_index);
                    }
                }
            } else {
                self.state.admin_deny_message(admin_player_index);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn shadowmute_player(
        &mut self,
        admin_player_index: PlayerIndex,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self.state.players.get_player(admin_player_index) {
            if admin_player.is_admin {
                let admin_player_name = admin_player.player_name.clone();

                if let Some(mute_player) = self.state.players.get_player_mut(mute_player_index) {
                    let old_status = mute_player.is_muted;
                    mute_player.is_muted = MuteStatus::ShadowMuted;
                    info!(
                        "{} ({}) shadowmuted {} ({})",
                        admin_player_name,
                        admin_player_index,
                        mute_player.player_name,
                        mute_player_index
                    );
                    let msg = format!(
                        "{} shadowmuted by {}",
                        mute_player.player_name, admin_player_name
                    );
                    if old_status == MuteStatus::Muted {
                        // Fake "unmuting" message
                        let msg = format!(
                            "{} unmuted by {}",
                            mute_player.player_name, admin_player_name
                        );
                        self.state
                            .add_directed_server_chat_message(msg, mute_player_index);
                    }
                    self.state
                        .add_directed_server_chat_message(msg, admin_player_index);
                }
            } else {
                self.state.admin_deny_message(admin_player_index);
            }
        }
    }

    pub(crate) fn mute_chat(&mut self, player_index: PlayerIndex) {
        if let Some(player) = self.state.players.get_player(player_index) {
            if player.is_admin {
                self.is_muted = true;

                let msg = format!("Chat muted by {}", player.player_name);
                info!("{} ({}) muted chat", player.player_name, player_index);
                self.state.add_server_chat_message(msg);
            } else {
                self.state.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn unmute_chat(&mut self, player_index: PlayerIndex) {
        if let Some(player) = self.state.players.get_player(player_index) {
            if player.is_admin {
                self.is_muted = false;

                let msg = format!("Chat unmuted by {}", player.player_name);
                info!("{} ({}) unmuted chat", player.player_name, player_index);

                self.state.add_server_chat_message(msg);
            } else {
                self.state.admin_deny_message(player_index);
            }
        }
    }

    pub(crate) fn admin_login(&mut self, player_index: PlayerIndex, password: &str) {
        if let Some(player) = self.state.players.get_player_mut(player_index) {
            let msg = if player.is_admin {
                "You are already logged in as administrator"
            } else if self.config.password == password {
                player.is_admin = true;
                info!("{} ({}) is now admin", player.player_name, player_index);
                "Successfully logged in as administrator"
            } else {
                info!(
                    "{} ({}) tried to become admin, entered wrong password",
                    player.player_name, player_index
                );
                "Wrong administrator password"
            };
            self.state
                .add_directed_server_chat_message(msg, player_index);
        }
    }

    pub(crate) fn restart_server(&mut self, player_index: PlayerIndex) {
        if let Some(player) = self.state.players.get_player(player_index) {
            if let Some(server_service) = self.config.server_service.as_deref() {
                if player.is_admin {
                    let msg = format!("{} started server restart", player.player_name);
                    self.state.add_server_chat_message(msg);
                    if let Err(_) = restart(server_service) {
                        self.state
                            .add_directed_server_chat_message("Restart failed", player_index);
                    }
                } else {
                    self.state.admin_deny_message(player_index);
                }
            }
        }
    }

    pub(crate) fn kick_all_matching<B: GameMode>(
        &mut self,
        admin_player_index: PlayerIndex,
        kick_player_name: &str,
        ban_player: bool,
        behaviour: &mut B,
    ) {
        if let Some(player) = self.state.players.get_player(admin_player_index) {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                enum Matching<'a> {
                    StartsWith(&'a str),
                    EndsWith(&'a str),
                    Contains(&'a str),
                    Equals(&'a str),
                }

                impl<'a> Matching<'a> {
                    fn is_matching(&self, player_name: &str) -> bool {
                        match self {
                            Matching::StartsWith(s) => player_name.starts_with(s),
                            Matching::EndsWith(s) => player_name.ends_with(s),
                            Matching::Contains(s) => player_name.contains(s),
                            Matching::Equals(s) => player_name == *s,
                        }
                    }
                }

                // 0 full string | 1 begins with | 2 ends with | 3 contains
                let matching = if kick_player_name.starts_with("%") {
                    if kick_player_name.ends_with("%") {
                        Matching::Contains(&kick_player_name[1..kick_player_name.len() - 1])
                    } else {
                        Matching::StartsWith(&kick_player_name[1..kick_player_name.len()])
                    }
                } else if kick_player_name.ends_with("%") {
                    Matching::EndsWith(&kick_player_name[0..kick_player_name.len() - 1])
                } else {
                    Matching::Equals(&kick_player_name)
                };

                let kick_player_list: Vec<_> = self
                    .state
                    .players
                    .iter_players()
                    .filter_map(|(player_index, player)| {
                        if let ServerPlayerData::NetworkPlayer { data } = &player.data {
                            if matching.is_matching(&player.player_name) {
                                return Some((player_index, player.player_name.clone(), data.addr));
                            }
                        }
                        None
                    })
                    .collect();

                if !kick_player_list.is_empty() {
                    for (player_index, player_name, player_addr) in kick_player_list {
                        if player_index != admin_player_index {
                            behaviour.before_player_exit(
                                self.into(),
                                player_index,
                                ExitReason::AdminKicked,
                            );
                            self.remove_player(player_index, true);

                            if ban_player {
                                self.ban.insert(player_addr.ip());

                                info!(
                                    "{} ({}) banned {} ({})",
                                    admin_player_name,
                                    admin_player_index,
                                    player_name,
                                    player_index
                                );
                                let msg =
                                    format!("{} banned by {}", player_name, admin_player_name);
                                self.state.add_server_chat_message(msg);
                            } else {
                                info!(
                                    "{} ({}) kicked {} ({})",
                                    admin_player_name,
                                    admin_player_index,
                                    player_name,
                                    player_index
                                );
                                let msg =
                                    format!("{} kicked by {}", player_name, admin_player_name);
                                self.state.add_server_chat_message(msg);
                            }
                        } else {
                            if ban_player {
                                self.state.add_directed_server_chat_message(
                                    "You cannot ban yourself",
                                    admin_player_index,
                                );
                            } else {
                                self.state.add_directed_server_chat_message(
                                    "You cannot kick yourself",
                                    admin_player_index,
                                );
                            }
                        }
                    }
                } else {
                    match matching {
                        Matching::Equals(_) => {
                            // full string
                            let msg = format!("No player names match {}", kick_player_name);
                            self.state
                                .add_directed_server_chat_message(msg, admin_player_index);
                        }
                        Matching::StartsWith(_) => {
                            // begins with%
                            let msg = format!("No player names begin with {}", kick_player_name);
                            self.state
                                .add_directed_server_chat_message(msg, admin_player_index);
                        }
                        Matching::EndsWith(_) => {
                            // %ends with
                            let msg = format!("No player names end with {}", kick_player_name);
                            self.state
                                .add_directed_server_chat_message(msg, admin_player_index);
                        }
                        Matching::Contains(_) => {
                            // %contains%
                            let msg = format!("No player names contain {}", kick_player_name);
                            self.state
                                .add_directed_server_chat_message(msg, admin_player_index);
                        }
                    }
                }
            } else {
                self.state.admin_deny_message(admin_player_index);
                return;
            }
        }
    }

    pub(crate) fn kick_player<B: GameMode>(
        &mut self,
        admin_player_index: PlayerIndex,
        kick_player_index: PlayerIndex,
        ban_player: bool,
        behaviour: &mut B,
    ) {
        if let Some(player) = self.state.players.get_player(admin_player_index) {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                if kick_player_index != admin_player_index {
                    if let Some(kick_player) = self.state.players.get_player(kick_player_index) {
                        if let ServerPlayerData::NetworkPlayer { data } = &kick_player.data {
                            let kick_player_name = kick_player.player_name.clone();
                            let kick_ip = data.addr.ip().clone();
                            behaviour.before_player_exit(
                                self.into(),
                                kick_player_index,
                                ExitReason::AdminKicked,
                            );
                            self.remove_player(kick_player_index, true);

                            if ban_player {
                                self.ban.insert(kick_ip);

                                info!(
                                    "{} ({}) banned {} ({})",
                                    admin_player_name,
                                    admin_player_index,
                                    kick_player_name,
                                    kick_player_name
                                );
                                let msg =
                                    format!("{} banned by {}", kick_player_name, admin_player_name);
                                self.state.add_server_chat_message(msg);
                            } else {
                                info!(
                                    "{} ({}) kicked {} ({})",
                                    admin_player_name,
                                    admin_player_index,
                                    kick_player_name,
                                    kick_player_name
                                );
                                let msg =
                                    format!("{} kicked by {}", kick_player_name, admin_player_name);
                                self.state.add_server_chat_message(msg);
                            }
                        }
                    }
                } else {
                    if ban_player {
                        self.state.add_directed_server_chat_message(
                            "You cannot ban yourself",
                            admin_player_index,
                        );
                    } else {
                        self.state.add_directed_server_chat_message(
                            "You cannot kick yourself",
                            admin_player_index,
                        );
                    }
                }
            } else {
                self.state.admin_deny_message(admin_player_index);
                return;
            }
        }
    }

    pub(crate) fn clear_bans(&mut self, player_index: PlayerIndex) {
        if let Some(player) = self.state.players.get_player(player_index) {
            if player.is_admin {
                self.ban.clear();
                info!("{} ({}) cleared bans", player.player_name, player_index);

                let msg = format!("Bans cleared by {}", player.player_name);
                self.state.add_server_chat_message(msg);
            } else {
                self.state.admin_deny_message(player_index);
            }
        }
    }

    pub fn set_replay(&mut self, player_index: PlayerIndex, rule: &str) {
        if let Some(player) = self.state.players.get_player(player_index) {
            if player.is_admin {
                match rule {
                    "on" => {
                        self.config.replays_enabled = ReplayEnabled::On;

                        info!("{} ({}) enabled replays", player.player_name, player_index);
                        let msg = format!("Replays enabled by {}", player.player_name);

                        self.state.add_server_chat_message(msg);
                    }
                    "off" => {
                        self.config.replays_enabled = ReplayEnabled::Off;

                        info!(
                            "{} ({}) disabled replay recording",
                            player.player_name, player_index
                        );
                        let msg = format!("Replays disabled by {}", player.player_name);

                        self.state.add_server_chat_message(msg);
                    }
                    "standby" => {
                        self.config.replays_enabled = ReplayEnabled::Standby;

                        info!(
                            "{} ({}) enabled standby replay recording",
                            player.player_name, player_index
                        );
                        let msg =
                            format!("Standby replay recording enabled by {}", player.player_name);

                        self.state.add_server_chat_message(msg);
                    }
                    _ => {}
                }
            } else {
                self.state.admin_deny_message(player_index);
            }
        }
    }
}
