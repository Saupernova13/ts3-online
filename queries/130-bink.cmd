# Level start hold: the Bink movie player (intro movies block the game loop)
maxlen 5000
symonly BinkOpen
symonly BinkClose
symonly BinkWait
strre (?i).*(\.bk2|movie|bink).*
