use crate::server::{HQMServer, MuteStatus, PlayerListExt, ServerPlayerData};

use crate::ReplayRecording;
use crate::game::{PlayerId, PlayerIndex};
use crate::gamemode::{ExitReason, GameMode};
use tracing::info;

impl HQMServer {
    pub(crate) fn set_allow_join(&mut self, admin_player_id: PlayerId, allowed: bool) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            self.allow_join = allowed;

            if allowed {
                info!("{} ({}) enabled joins", player.player_name, admin_player_id);
                let msg = format!("Joins enabled by {}", player.player_name);
                self.state.players.add_server_chat_message(msg);
            } else {
                info!(
                    "{} ({}) disabled joins",
                    player.player_name, admin_player_id
                );
                let msg = format!("Joins disabled by {}", player.player_name);
                self.state.players.add_server_chat_message(msg);
            }
        }
    }

    pub(crate) fn mute_player(
        &mut self,
        admin_player_id: PlayerId,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = admin_player.player_name.clone();

            if let Some((mute_player_id, mute_player)) = self
                .state
                .players
                .players
                .get_player_mut_by_index(mute_player_index)
            {
                mute_player.is_muted = MuteStatus::Muted;
                info!(
                    "{} ({}) muted {} ({})",
                    admin_player_name, admin_player_id, mute_player.player_name, mute_player_id
                );
                let msg = format!("{} muted by {}", mute_player.player_name, admin_player_name);
                self.state.players.add_server_chat_message(msg);
            }
        }
    }

    pub(crate) fn unmute_player(
        &mut self,
        admin_player_id: PlayerId,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = admin_player.player_name.clone();

            if let Some((mute_player_id, mute_player)) = self
                .state
                .players
                .players
                .get_player_mut_by_index(mute_player_index)
            {
                let old_status = mute_player.is_muted;
                mute_player.is_muted = MuteStatus::NotMuted;
                info!(
                    "{} ({}) unmuted {} ({})",
                    admin_player_name, admin_player_id, mute_player.player_name, mute_player_id
                );
                let msg = format!(
                    "{} unmuted by {}",
                    mute_player.player_name, admin_player_name
                );
                if old_status == MuteStatus::Muted {
                    self.state.players.add_server_chat_message(msg);
                } else {
                    self.state
                        .players
                        .add_directed_server_chat_message(msg, admin_player_id);
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn shadowmute_player(
        &mut self,
        admin_player_id: PlayerId,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = admin_player.player_name.clone();

            if let Some((mute_player_id, mute_player)) = self
                .state
                .players
                .players
                .get_player_mut_by_index(mute_player_index)
            {
                let old_status = mute_player.is_muted;
                mute_player.is_muted = MuteStatus::ShadowMuted;
                info!(
                    "{} ({}) shadowmuted {} ({})",
                    admin_player_name, admin_player_id, mute_player.player_name, mute_player_id
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
                        .players
                        .add_directed_server_chat_message(msg, mute_player_id);
                }
                self.state
                    .players
                    .add_directed_server_chat_message(msg, admin_player_id);
            }
        }
    }

    pub(crate) fn mute_chat(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            self.is_muted = true;

            let msg = format!("Chat muted by {}", player.player_name);
            info!("{} ({}) muted chat", player.player_name, admin_player_id);
            self.state.players.add_server_chat_message(msg);
        }
    }

    pub(crate) fn unmute_chat(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            self.is_muted = false;

            let msg = format!("Chat unmuted by {}", player.player_name);
            info!("{} ({}) unmuted chat", player.player_name, admin_player_id);

            self.state.players.add_server_chat_message(msg);
        }
    }

    pub(crate) fn admin_login(&mut self, player_id: PlayerId, password: &str) {
        if let Some(player) = self.state.players.players.get_player_mut(player_id) {
            let msg = if player.is_admin {
                "You are already logged in as administrator"
            } else if self
                .config
                .password
                .as_deref()
                .is_some_and(|x| x == password)
            {
                player.is_admin = true;
                info!("{} ({}) is now admin", player.player_name, player_id);
                "Successfully logged in as administrator"
            } else {
                info!(
                    "{} ({}) tried to become admin, entered wrong password",
                    player.player_name, player_id
                );
                "Wrong administrator password"
            };
            self.state
                .players
                .add_directed_server_chat_message(msg, player_id);
        }
    }

    pub(crate) fn restart_server(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            if let Some(server_service) = self.config.server_service.as_deref() {
                let msg = format!("{} started server restart", player.player_name);
                self.state.players.add_server_chat_message(msg);
                let ctl = systemctl::SystemCtl::default();
                if let Err(_) = ctl.restart(server_service) {
                    self.state
                        .players
                        .add_directed_server_chat_message("Restart failed", admin_player_id);
                }
            }
        }
    }

    pub(crate) fn kick_all_matching<B: GameMode>(
        &mut self,
        admin_player_id: PlayerId,
        kick_player_name: &str,
        ban_player: bool,
        behaviour: &mut B,
    ) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
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
                for (player_id, player_name, player_addr) in kick_player_list {
                    if player_id != admin_player_id {
                        behaviour.before_player_exit(
                            self.into(),
                            player_id,
                            ExitReason::AdminKicked,
                        );
                        self.remove_player(player_id, true);

                        if ban_player {
                            self.ban.ban_ip(player_addr.ip());

                            info!(
                                "{} ({}) banned {} ({})",
                                admin_player_name, admin_player_id, player_name, player_id
                            );
                            let msg = format!("{} banned by {}", player_name, admin_player_name);
                            self.state.players.add_server_chat_message(msg);
                        } else {
                            info!(
                                "{} ({}) kicked {} ({})",
                                admin_player_name, admin_player_id, player_name, player_id
                            );
                            let msg = format!("{} kicked by {}", player_name, admin_player_name);
                            self.state.players.add_server_chat_message(msg);
                        }
                    } else {
                        if ban_player {
                            self.state.players.add_directed_server_chat_message(
                                "You cannot ban yourself",
                                admin_player_id,
                            );
                        } else {
                            self.state.players.add_directed_server_chat_message(
                                "You cannot kick yourself",
                                admin_player_id,
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
                            .players
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                    Matching::StartsWith(_) => {
                        // begins with%
                        let msg = format!("No player names begin with {}", kick_player_name);
                        self.state
                            .players
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                    Matching::EndsWith(_) => {
                        // %ends with
                        let msg = format!("No player names end with {}", kick_player_name);
                        self.state
                            .players
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                    Matching::Contains(_) => {
                        // %contains%
                        let msg = format!("No player names contain {}", kick_player_name);
                        self.state
                            .players
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                }
            }
        }
    }

    pub(crate) fn kick_player<B: GameMode>(
        &mut self,
        admin_player_id: PlayerId,
        kick_player_index: PlayerIndex,
        ban_player: bool,
        behaviour: &mut B,
    ) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = player.player_name.clone();

            if kick_player_index != admin_player_id.index {
                if let Some((kick_player_id, kick_player)) = self
                    .state
                    .players
                    .players
                    .get_player_by_index(kick_player_index)
                {
                    if let ServerPlayerData::NetworkPlayer { data } = &kick_player.data {
                        let kick_player_name = kick_player.player_name.clone();
                        let kick_ip = data.addr.ip().clone();
                        behaviour.before_player_exit(
                            self.into(),
                            kick_player_id,
                            ExitReason::AdminKicked,
                        );
                        self.remove_player(kick_player_id, true);

                        if ban_player {
                            self.ban.ban_ip(kick_ip);

                            info!(
                                "{} ({}) banned {} ({})",
                                admin_player_name,
                                admin_player_id,
                                kick_player_name,
                                kick_player_id
                            );
                            let msg =
                                format!("{} banned by {}", kick_player_name, admin_player_name);
                            self.state.players.add_server_chat_message(msg);
                        } else {
                            info!(
                                "{} ({}) kicked {} ({})",
                                admin_player_name,
                                admin_player_id,
                                kick_player_name,
                                kick_player_id
                            );
                            let msg =
                                format!("{} kicked by {}", kick_player_name, admin_player_name);
                            self.state.players.add_server_chat_message(msg);
                        }
                    }
                }
            } else {
                if ban_player {
                    self.state.players.add_directed_server_chat_message(
                        "You cannot ban yourself",
                        admin_player_id,
                    );
                } else {
                    self.state.players.add_directed_server_chat_message(
                        "You cannot kick yourself",
                        admin_player_id,
                    );
                }
            }
        }
    }

    pub(crate) fn clear_bans(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            self.ban.clear_all_bans();
            info!("{} ({}) cleared bans", player.player_name, admin_player_id);

            let msg = format!("Bans cleared by {}", player.player_name);
            self.state.players.add_server_chat_message(msg);
        }
    }

    pub fn set_recording(&mut self, admin_player_id: PlayerId, rule: &str) {
        if let Some(player) = self
            .state
            .players
            .players
            .check_admin_or_deny(admin_player_id)
        {
            match rule {
                "on" => {
                    self.config.recording_enabled = ReplayRecording::On;

                    info!(
                        "{} ({}) enabled replays",
                        player.player_name, admin_player_id
                    );
                    let msg = format!("Replays enabled by {}", player.player_name);

                    self.state.players.add_server_chat_message(msg);
                }
                "off" => {
                    self.config.recording_enabled = ReplayRecording::Off;

                    info!(
                        "{} ({}) disabled replay recording",
                        player.player_name, admin_player_id
                    );
                    let msg = format!("Replays disabled by {}", player.player_name);

                    self.state.players.add_server_chat_message(msg);
                }
                "standby" => {
                    self.config.recording_enabled = ReplayRecording::Standby;

                    info!(
                        "{} ({}) enabled standby replay recording",
                        player.player_name, admin_player_id
                    );
                    let msg = format!("Standby replay recording enabled by {}", player.player_name);

                    self.state.players.add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }
}
