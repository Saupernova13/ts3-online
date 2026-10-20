# Level start hold: the pause handlers, the game timer 0x140954820 and its speed
maxlen 5000
strre ^(Unpause|UnPause|Resume|RequestUnpauseSpeed|SyncTimer)$
func 0x140479c00
func 0x1404779d0
func 0x1404c7620
func 0x140270530
refsonly 0x140954820
refsonly 0x140b2c5a6
