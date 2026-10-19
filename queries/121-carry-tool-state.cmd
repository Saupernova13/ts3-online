# Player state sync: forcing a carry (PlayerPerception) and the current tool (SelectNewTool / ToolChanged)
maxlen 5000
func 140056b30
str SelectNewTool
str SelectNewTool_FromCode
str ToolChanged
strre ^(PlayerPerception_.*|Tool_.*|.*CurrentTool.*|.*SetTool.*)$
