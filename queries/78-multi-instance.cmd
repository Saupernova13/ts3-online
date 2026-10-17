# Host/client feasibility: single-instance mutex, window class + WndProc (focus handling),
# Steam relaunch, and the actor set-transform used by the placing handler
maxlen 7000
symonly CreateMutexA
symonly RegisterClassA
symonly SteamAPI_RestartAppIfNecessary
func 140568b50
