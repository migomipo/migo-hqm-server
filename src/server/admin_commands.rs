use crate::server::HQMServer;

use crate::ReplayRecording;
use crate::game::{PlayerId, PlayerIndex};
use crate::gamemode::{ExitReason, GameMode};
use crate::players::{MuteStatus, PlayerListExt, ServerPlayerData};
use tracing::info;

impl HQMServer {
    pub(crate) fn mute_player(
        &mut self,
        admin_player_id: PlayerId,
        mute_player_index: PlayerIndex,
    ) {
        if let Some(admin_player) = self
            .state
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = admin_player.player_name.clone();

            if let Some((mute_player_id, mute_player)) = self
                .state
                .player_message_state
                .players
                .get_player_mut_by_index(mute_player_index)
            {
                mute_player.is_muted = MuteStatus::Muted;
                info!(
                    "{} ({}) muted {} ({})",
                    admin_player_name, admin_player_id, mute_player.player_name, mute_player_id
                );
                let msg = format!("{} muted by {}", mute_player.player_name, admin_player_name);
                self.state.player_message_state.add_server_chat_message(msg);
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
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = admin_player.player_name.clone();

            if let Some((mute_player_id, mute_player)) = self
                .state
                .player_message_state
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
                    self.state.player_message_state.add_server_chat_message(msg);
                } else {
                    self.state
                        .player_message_state
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
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = admin_player.player_name.clone();

            if let Some((mute_player_id, mute_player)) = self
                .state
                .player_message_state
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
                        .player_message_state
                        .add_directed_server_chat_message(msg, mute_player_id);
                }
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(msg, admin_player_id);
            }
        }
    }

    pub(crate) fn mute_chat(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            self.is_muted = true;

            let msg = format!("Chat muted by {}", player.player_name);
            info!("{} ({}) muted chat", player.player_name, admin_player_id);
            self.state.player_message_state.add_server_chat_message(msg);
        }
    }

    pub(crate) fn unmute_chat(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            self.is_muted = false;

            let msg = format!("Chat unmuted by {}", player.player_name);
            info!("{} ({}) unmuted chat", player.player_name, admin_player_id);

            self.state.player_message_state.add_server_chat_message(msg);
        }
    }

    pub(crate) fn admin_login(&mut self, player_id: PlayerId, password: &str) {
        if let Some(player) = self
            .state
            .player_message_state
            .players
            .get_player_mut(player_id)
        {
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
                .player_message_state
                .add_directed_server_chat_message(msg, player_id);
        }
    }

    pub(crate) fn restart_server(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            if let Some(server_service) = self.config.server_service.as_deref() {
                let msg = format!("{} started server restart", player.player_name);
                self.state.player_message_state.add_server_chat_message(msg);
                let ctl = systemctl::SystemCtl::default();
                if ctl.restart(server_service).is_err() {
                    self.state
                        .player_message_state
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
            .player_message_state
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
                    let player_name = player_name.to_lowercase();
                    match self {
                        Matching::StartsWith(s) => player_name.starts_with(s),
                        Matching::EndsWith(s) => player_name.ends_with(s),
                        Matching::Contains(s) => player_name.contains(s),
                        Matching::Equals(s) => player_name == *s,
                    }
                }
            }

            // Names match case-insensitively, and % is a wildcard at the start or end.
            let pattern = kick_player_name.to_lowercase();
            let (pattern, any_start) = match pattern.strip_prefix('%') {
                Some(rest) => (rest, true),
                None => (pattern.as_str(), false),
            };
            let (pattern, any_end) = match pattern.strip_suffix('%') {
                Some(rest) => (rest, true),
                None => (pattern, false),
            };
            let matching = match (any_start, any_end) {
                (true, true) => Matching::Contains(pattern),
                (true, false) => Matching::EndsWith(pattern),
                (false, true) => Matching::StartsWith(pattern),
                (false, false) => Matching::Equals(pattern),
            };

            let kick_player_list: Vec<_> = self
                .state
                .player_message_state
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
                            &mut self.context(),
                            player_id,
                            ExitReason::AdminKicked,
                        );
                        self.state.remove_player(player_id, true);

                        if ban_player {
                            self.ban.ban_ip(player_addr.ip());

                            info!(
                                "{} ({}) banned {} ({})",
                                admin_player_name, admin_player_id, player_name, player_id
                            );
                            let msg = format!("{player_name} banned by {admin_player_name}");
                            self.state.player_message_state.add_server_chat_message(msg);
                        } else {
                            info!(
                                "{} ({}) kicked {} ({})",
                                admin_player_name, admin_player_id, player_name, player_id
                            );
                            let msg = format!("{player_name} kicked by {admin_player_name}");
                            self.state.player_message_state.add_server_chat_message(msg);
                        }
                    } else if ban_player {
                        self.state
                            .player_message_state
                            .add_directed_server_chat_message(
                                "You cannot ban yourself",
                                admin_player_id,
                            );
                    } else {
                        self.state
                            .player_message_state
                            .add_directed_server_chat_message(
                                "You cannot kick yourself",
                                admin_player_id,
                            );
                    }
                }
            } else {
                match matching {
                    Matching::Equals(_) => {
                        // full string
                        let msg = format!("No player names match {kick_player_name}");
                        self.state
                            .player_message_state
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                    Matching::StartsWith(_) => {
                        // begins with%
                        let msg = format!("No player names begin with {kick_player_name}");
                        self.state
                            .player_message_state
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                    Matching::EndsWith(_) => {
                        // %ends with
                        let msg = format!("No player names end with {kick_player_name}");
                        self.state
                            .player_message_state
                            .add_directed_server_chat_message(msg, admin_player_id);
                    }
                    Matching::Contains(_) => {
                        // %contains%
                        let msg = format!("No player names contain {kick_player_name}");
                        self.state
                            .player_message_state
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
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = player.player_name.clone();

            if kick_player_index != admin_player_id.index {
                if let Some((kick_player_id, kick_player)) = self
                    .state
                    .player_message_state
                    .players
                    .get_player_by_index(kick_player_index)
                {
                    if let ServerPlayerData::NetworkPlayer { data } = &kick_player.data {
                        let kick_player_name = kick_player.player_name.clone();
                        let kick_ip = data.addr.ip();
                        behaviour.before_player_exit(
                            &mut self.context(),
                            kick_player_id,
                            ExitReason::AdminKicked,
                        );
                        self.state.remove_player(kick_player_id, true);

                        if ban_player {
                            self.ban.ban_ip(kick_ip);

                            info!(
                                "{} ({}) banned {} ({})",
                                admin_player_name,
                                admin_player_id,
                                kick_player_name,
                                kick_player_id
                            );
                            let msg = format!("{kick_player_name} banned by {admin_player_name}");
                            self.state.player_message_state.add_server_chat_message(msg);
                        } else {
                            info!(
                                "{} ({}) kicked {} ({})",
                                admin_player_name,
                                admin_player_id,
                                kick_player_name,
                                kick_player_id
                            );
                            let msg = format!("{kick_player_name} kicked by {admin_player_name}");
                            self.state.player_message_state.add_server_chat_message(msg);
                        }
                    }
                }
            } else if ban_player {
                self.state
                    .player_message_state
                    .add_directed_server_chat_message("You cannot ban yourself", admin_player_id);
            } else {
                self.state
                    .player_message_state
                    .add_directed_server_chat_message("You cannot kick yourself", admin_player_id);
            }
        }
    }

    pub(crate) fn clear_bans(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            self.ban.clear_all_bans();
            info!("{} ({}) cleared bans", player.player_name, admin_player_id);

            let msg = format!("Bans cleared by {}", player.player_name);
            self.state.player_message_state.add_server_chat_message(msg);
        }
    }

    /// Ends the current game. The new game starts once the command has been handled.
    pub(crate) fn reset_game(&mut self, admin_player_id: PlayerId) {
        if let Some(player) = self
            .state
            .player_message_state
            .check_admin_or_deny(admin_player_id)
        {
            let name = player.player_name.clone();
            info!("{} ({}) reset game", name, admin_player_id);
            self.context().new_game();
            let msg = format!("Game reset by {name}");
            self.state.player_message_state.add_server_chat_message(msg);
        }
    }

    pub fn set_recording(&mut self, admin_player_id: PlayerId, rule: &str) {
        if let Some(player) = self
            .state
            .player_message_state
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

                    self.state.player_message_state.add_server_chat_message(msg);
                }
                "off" => {
                    self.config.recording_enabled = ReplayRecording::Off;

                    info!(
                        "{} ({}) disabled replay recording",
                        player.player_name, admin_player_id
                    );
                    let msg = format!("Replays disabled by {}", player.player_name);

                    self.state.player_message_state.add_server_chat_message(msg);
                }
                "standby" => {
                    self.config.recording_enabled = ReplayRecording::Standby;

                    info!(
                        "{} ({}) enabled standby replay recording",
                        player.player_name, admin_player_id
                    );
                    let msg = format!("Standby replay recording enabled by {}", player.player_name);

                    self.state.player_message_state.add_server_chat_message(msg);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamemode::standard_match::test_game::{MatchGame, match_server};
    use crate::gamemode::util::SpawnPoint;
    use crate::gamemode::warmup::PermanentWarmup;
    use crate::server::test_server::TestServer;
    use glam::Vec3;

    #[test]
    fn admin_logs_in_with_the_right_password() {
        let mut t = match_server();
        t.server.config.password = Some("secret".to_owned());
        let player = t.join("player");
        t.take_messages(player);
        t.chat(player, "/admin wrong");
        assert_eq!(t.take_chats(player), ["Wrong administrator password"]);
        t.chat(player, "/admin secret");
        assert_eq!(
            t.take_chats(player),
            ["Successfully logged in as administrator"]
        );
        t.chat(player, "/mutechat");
        assert_eq!(t.take_chats(player), ["Chat muted by player"]);
    }

    #[test]
    fn kick_removes_a_player() {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        let kicked = t.join("kicked");
        t.take_messages(admin);
        t.chat(admin, "/kick 1");
        assert!(t.ctx().players.get(kicked).is_none());
        assert!(
            t.take_chats(admin)
                .contains(&"kicked kicked by admin".to_owned())
        );
    }

    #[test]
    fn ban_kicks_and_bans_until_clearbans() {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        let banned = t.join("banned");
        t.chat(admin, "/ban 1");
        assert!(t.ctx().players.get(banned).is_none());
        // Test players all connect from 127.0.0.1, so nobody else can join while it is banned.
        assert!(t.try_join("again").is_none());
        t.chat(admin, "/clearbans");
        assert!(t.try_join("again").is_some());
    }

    #[test]
    fn mute_and_unmute_a_player() {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        let player = t.join("player");
        t.chat(admin, "/mute 1");
        t.take_messages(admin);
        t.chat(player, "hello");
        assert!(!t.take_chats(admin).contains(&"hello".to_owned()));
        t.chat(admin, "/unmute 1");
        t.chat(player, "hello again");
        assert!(t.take_chats(admin).contains(&"hello again".to_owned()));
    }

    #[test]
    fn mutechat_and_unmutechat_mute_all_chat() {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        let player = t.join("player");
        t.chat(admin, "/mutechat");
        t.take_messages(admin);
        t.chat(player, "hello");
        assert!(!t.take_chats(admin).contains(&"hello".to_owned()));
        t.chat(admin, "/unmutechat");
        t.chat(player, "hello again");
        assert!(t.take_chats(admin).contains(&"hello again".to_owned()));
    }

    #[test]
    fn reset_resets_the_game() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        game.score(game.red[0]);
        game.chat(game.admin(), "/reset");
        game.ticks(2);
        assert_eq!(game.scoreboard.period, 0);
        assert_eq!(game.scoreboard.red_score, 0);
        assert!(game.heard("Game reset by red0"), "{:?}", game.heard);
    }

    #[test]
    fn reset_works_in_every_game_mode() {
        let mut t = TestServer::new(PermanentWarmup::new(1, SpawnPoint::Center));
        let admin = t.join_admin("admin");
        t.take_messages(admin);
        t.tick();
        let start = |t: &mut TestServer<PermanentWarmup>| {
            t.ctx().objects.pucks().next().unwrap().1.body.pos
        };
        let puck_start = start(&mut t);
        t.ctx().objects.pucks_mut().next().unwrap().1.body.pos = Vec3::new(5.0, 1.0, 5.0);
        t.chat(admin, "/reset");
        // The new game puts the warmup puck back where it starts.
        let puck = start(&mut t);
        assert_eq!((puck.x, puck.z), (puck_start.x, puck_start.z));
        assert_eq!(t.take_chats(admin), ["Game reset by admin"]);
    }

    #[test]
    fn replay_turns_server_side_replays_on_and_off() {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        t.take_messages(admin);
        t.chat(admin, "/replay on");
        assert_eq!(t.server.config.recording_enabled, ReplayRecording::On);
        t.chat(admin, "/replay off");
        assert_eq!(t.server.config.recording_enabled, ReplayRecording::Off);
        assert_eq!(
            t.take_chats(admin),
            ["Replays enabled by admin", "Replays disabled by admin"]
        );
    }

    /// Kicks with `/kickall pattern` from a server with MigoMipo and Other, and returns the patterns
    /// that did not kick exactly MigoMipo.
    fn kickall_failures(patterns: &[&'static str]) -> Vec<&'static str> {
        let mut failures = vec![];
        for &pattern in patterns {
            let mut t = match_server();
            let admin = t.join_admin("admin");
            let target = t.join("MigoMipo");
            let other = t.join("Other");
            t.chat(admin, &format!("/kickall {pattern}"));
            if t.ctx().players.get(target).is_some() || t.ctx().players.get(other).is_none() {
                failures.push(pattern);
            }
        }
        failures
    }

    #[test]
    fn kickall_kicks_players_with_the_exact_name() {
        assert_eq!(kickall_failures(&["MigoMipo"]), Vec::<&str>::new());
    }

    #[test]
    fn kickall_ignores_case_and_supports_wildcards() {
        let patterns = ["migomipo", "Migo%", "migo%", "%Mipo", "%mipo", "%gomi%"];
        let failures = kickall_failures(&patterns);
        assert!(
            failures.is_empty(),
            "patterns that did not kick only MigoMipo: {failures:?}"
        );
    }

    #[test]
    fn banall_kicks_and_bans_players_by_name() {
        let mut t = match_server();
        let admin = t.join_admin("admin");
        let target = t.join("MigoMipo");
        t.chat(admin, "/banall MigoMipo");
        assert!(t.ctx().players.get(target).is_none());
        assert!(t.try_join("again").is_none());
    }
}
