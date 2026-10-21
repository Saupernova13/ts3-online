# Turning an actor's AI off: script bindings and the behaviour control component
maxlen 3000
strre ^(Ai|AI)[A-Za-z_]*(Enable|Disable|Suspend|Pause|Resume|Stop|Start|Active|Freeze)[A-Za-z_]*$
strre ^[A-Za-z_]*(Enable|Disable|Suspend|Freeze|Pause)(Ai|AI|Brain|Behavior|Behaviour)[A-Za-z_]*$
vt AiBehaviorControl
