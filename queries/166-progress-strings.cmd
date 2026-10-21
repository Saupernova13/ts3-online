# Progress the client must follow: missions (challenges), catalog purchases, buildings, checkpoints, level restart
strre (?i)^(challenge|mission|catalog|building|checkpoint|restart|retry|reload|progress|unlock|gamedata|savedata|persist).{0,60}$
strre (?i).{0,30}(ChallengeComplete|MissionComplete|ChallengePass|ChallengeFail|LevelRestart|RestartLevel|RestartCheckpoint|ReloadCheckpoint|Checkpoint).{0,40}
