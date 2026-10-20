# Host/client feasibility: instance mutex, WndProc focus, Steam relaunch, actor set-transform
maxlen 7000
symonly CreateMutexA
symonly RegisterClassA
symonly SteamAPI_RestartAppIfNecessary
func 140568b50
