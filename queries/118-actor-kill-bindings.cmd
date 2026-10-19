# World sync: script bindings that kill, damage, hide or move a named actor (to replay host events)
maxlen 3000
strre (?i)^(actor_|)(kill|destroy|remove|explode|break|damage|despawn|setposition|setpos|teleport|hide|show|activate|deactivate|enable|disable)[a-z_]*$
strre (?i).*(killactor|destroyactor|explodeactor|damageactor|actorsetpos|setactorpos|teleportactor).*
