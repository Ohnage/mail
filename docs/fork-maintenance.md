# Fork maintenance

The fork tracks `https://github.com/flectar/mail` main. Integrate upstream before feature work, preserving fork settings and migrations; do not reset the fork to upstream or replace user configuration.

Before a build, fetch upstream and run `python scripts/check-fork-upstream.py`. The check reads current upstream/main and verifies it is an ancestor of the build commit. On this fork, a new upstream commit warns and lets a push or manual artifact build continue; a run that publishes a GitHub Release still fails until upstream is integrated. It does not merge code or update an installed app automatically.

Use normal fast-forward publication to fork main after focused regression checks and an integration build. Record the verified upstream SHA and fork SHA with the build. Keep account/tab branding and registry migration tests when resolving future upstream changes. Test migrations on synthetic or copied data and inspect the settings and tab editor in light/dark themes and narrow windows.

This change integrated upstream commit `b6fa4aae9b5058a5f28b702d39447dc3d8508c21` into fork base `93a9a23265126b23e18f737cc5dc5649ccd1f25f`. Recheck before publication and packaging; this recorded baseline is not a claim that upstream never advances.
