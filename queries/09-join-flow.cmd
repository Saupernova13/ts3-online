# How a second player joins: join-in screen, join events, controller binding
maxlen 12000
strre ^(JoinIn|ENTER_JOIN|WAIT_TO_JOIN|Scn_Join|IDG_AS_2P_JOIN|PlayerJoinDisallowed|PlayerJoinsMission|ForceAllJoinMultiplayer|SharedScreenModeChanged|ControlMapper|ScreenJoinIn|LoadingTip_Misc_2PlayerJoin)$
vt ScreenJoinIn
vt CoOpController
vt ControlMapper
