# Migo HQM Server

This is my own HQM server implementation. It attempts to recreate the physics as accurately as possible, while adding some features and bug fixes. Most notably, it fixes the goal detection glitch, allows for multiple pucks in warmups, has a completely revamped offside+icing implementation, server-side ping and some administrator features. Performance may also be slightly better and more consistent.

## How to start

You will need a configuration file to start, a default config.ini is provided. 

Run `migo-hqm-server` to start the server with config.ini in the current working directory, or `migo-hqm-server <path-to-config>` to run with any compatible configuration file in your system.

## How to configure

config.ini is a good starting point, and contains the important available settings. It is divided into three sections.

### Server

Property               | Explanation
-----------------------| -------------
name                   | Name of the server that will be visible in the server list
port                   | Port number, must be a number between 0 and 65535. 27585 is the default, and most servers are in the 27585-27599 range.
mode                   | Game mode. Currently supported values are "match" (play matches), "warmup" (warmup mode forever), "russian" (Russian 1v1/2v2) and "shootout" (shootout mode).
public                 | If true, the server will notify the master server so that clients can find this server easily in the server list.
log_name               | (optional) Log name prefix. Log files will end up in a "log" folder in the current working directory, and be named *log_name*-*date*. Default log name prefix is the server name + ".log".
team_max               | Number of players allowed in each team.
player_max             | Number of players allowed in the server.
force_team_size_parity | (optional) If true, players will not be able to join the team that already has more players than the other one. Default value is false.
password               | Administrator password.
welcome                | Welcome message that is sent to all players when they're joining. \n will create a new line. The client can only show 7 chat lines at a time, and it is not recommended to have more than three lines.
replays                | (optional) If true, all matches that start will be saved as replays. Games that ended before the warmup ended will not be saved.               

### Game

Property               | Explanation
-----------------------| -------------
spawn                  | Spawn point for players who join a team. Allowed values are "center" (default, spawns players at the center faceoff circle) and "bench" (spawns players right next to the "bench", opposite side of the spectator camera)
limit_jump_speed       | If true, nerfs jump speed, effectively nerfing double-jumping. If false, it should work like vanilla.
offside                | Offside setting. Allowed values are "off" (default, no offside), "on", (offside rule enabled) and "immediate", which will call offside immediately instead of warning when the puck has entered the offensive zone in an offside situation.
icing                  | Icing setting. Allowed values are "off" (default, no icing), "on" (touch icing rule enabled) and "notouch" (no-touch icing rule enabled)
time_period            | (Match mode only) Period length in seconds.
time_warmup            | (Match mode only) Warmup length in seconds.
time_intermission      | (Match mode only) Intermission length in seconds.
warmup_pucks           | Number of pucks in warmup. Only 32 objects (pucks+players) are allowed on the ice at the time, so at warmup there can never be more players than (32 minus number of pucks) on the ice.
mercy                  | (Match mode only) Mercy rule setting. If 0, mercy rule will be disabled. Otherwise, games will automatically end if a team scores and leads by at least X goals.
first                  | (Match mode only) First-to-goals rule setting. If 0, first-to-goals rule will be disabled. Otherwise, games will automatically end if a team scores and and reaches at least X goals scored.
attempts               | (Russian 1v1 or shootout mode only) How many attempts each team will get. Default is 5 for shootout mode and 10 for Russian 1v1 mode.

### Physics
Property                  | Explanation
--------------------------|----------------
gravity                   | Gravitational acceleration in meters per second squared. Default is 6.80555.
player_acceleration       | Player acceleration in meters per second squared. Default is 2.08333.
player_deceleration       | Player deceleration in meters per second squared. Default is 5.55555.
max_player_speed          | Maximum player speed in meters per second. Default is 5.
max_player_shift_speed    | Some shift-turning related maximum speed in meters per second. Default is 3.33333.
puck_rink_friction        | Friction rotation between puck and rink (both ice and boards). Default is 0.05.
player_turning            | Player turning acceleration in meters per second squared. Default is 4.1666666.
player_shift_turning      | Player shift-turning acceleration in meters per second squared. Default is 3.88888. 
player_shift_acceleration | Some shift-turning related acceleration in meters per second squared. Default is 2.7777.

## Commands

### Available for all

Commands               | Explanation
-----------------------|--------------
/list                  | Lists up to 5 player IDs. These IDs are used for a few other commands to uniquely determine a player.
/list *ID*             | Lists up to 5 player IDs, starting from *ID*, which must be a number.
/search *S*            | Lists up to 5 player IDs of players who have the substring S in their player name.
/view *ID*             | Enters first person view of player with ID *ID*. If you're on the ice, your player will be removed and you will become a spectator.
/views *S*             | Searches for player with name *S* and enters first person view of that player if a unique match is found. If multiple matches are found, they are listed instead.
/restoreview           | Restores first person view.
/ping *ID*             | Get server-side ping of player with ID *ID*
/pings *S*             | Searches for player with name *S* and gets server-side ping for that player if a unique match is found. If multiple matches are found, they are listed instead.
/lefty                 | Makes player left-handed. If done during play, it will only be applied after play has stopped.
/righty                | Makes player right-handed. If done during play, it will only be applied after play has stopped.
/rules                 | Shows current offside/icing rule settings.
/admin *PASSWORD*      | Logs in as administrator, if the password is correct.

### Administrators only

Commands                 | Explanation
-------------------------|--------------
/disablejoin             | Prevents new players from joining the server.
/enablejoin              | Enables new players to join the server.
/kick *ID*               | Kicks player with ID *ID*.
/ban *ID*                | Kicks and IP-bans player with ID *ID*.
/fs *ID*                 | Forces player with ID *ID* off ice.
/mute *ID*               | Mutes player with ID *ID*.
/unmute *ID*             | Unmutes player with ID *ID*.
/mutechat                | Mutes all chat.
/unmutechat              | Unmutes all chat, individual user chat mutes still apply.
/start                   | Starts game.
/reset                   | Resets game.
/pause                   | Pauses game.
/unpause                 | Unpauses game.
/faceoff                 | Calls center-ice faceoff.
/set clock *M*:*S*       | Sets game clock.
/set period *N*          | Sets period. OT1 is 4, OT2 is 5, etc. 0 is warmup.
/set redscore *N*        | Sets red score.
/set bluescore *N*       | Sets blue score.
/set icing *S*           | Sets icing rule. Allowed values are "off", "on" (touch icing" and "notouch" (no-touch icing)
/set offside *S*         | Sets offside rule. Allowed values are "off", "on" (delayed offside) and "imm" or "immediate" (immediate offside, no offside warnings).
/set teamsize *N*        | Sets team size.
/set teamparity *on/off* | If enabled, players will not be able to join the team that already has more players than the other one.
/set replay *on/off*     | Enables/disables server-side replays
/set mercy *N*           | Sets mercy rule setting. If 0, mercy rule will be disabled. Otherwise, games will automatically end if a team scores and leads by at least X goals.
/set first *N*           | Sets first-to-goals rule setting. If 0, first-to-goals rule will be disabled. Otherwise, games will automatically end if a team scores and and reaches at least X goals scored.
/kickall *S*             | Kicks all players with a player name equal to *S* (case-insensitive). % can be used as wildcards at the start and end of *S* to match players with similar names. For example, migo%, %mipo and %gomi% all match MigoMipo.
/banall *S*              | Same as /kickall, but also IP-bans.


