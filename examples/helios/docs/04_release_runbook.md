# Helios Release Runbook (the one people actually follow)

This is the runbook the on-call actually pulls up at 3am. Keep it current. (It is
not current. Nothing here is current. That is the point of this exercise.)

## Deploy

Deploys moved to Tuesday after we realized Wednesday collided with the weekly
all-hands and nobody was around to watch the graphs.

Steps:

1. Get approval (see ownership section — it changed, ask in channel).
2. Run `helios deploy --tenant all` from the bastion.
3. Watch the error rate for 30 minutes.

## Release

Releases happen on Monday. Customers expect the new build at the start of their
week and support staffs up for it.

## Rollback

If it's on fire, `helios rollback` and post in `#incident`. Then update this
runbook, which you will not do.
