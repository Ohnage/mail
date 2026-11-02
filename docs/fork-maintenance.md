# Fork maintenance

The fork tracks `https://github.com/flectar/mail` main. Integrate upstream before feature work, preserving fork settings and migrations; do not reset the fork to upstream or replace user configuration.

Before a build, fetch upstream and run `python scripts/check-fork-upstream.py`. The check reads current upstream/main and verifies it is an ancestor of the build commit. A new upstream commit stops the fork main Windows artifact preflight until it is integrated. It does not merge code or update an installed app automatically.

Use normal fast-forward publication to fork main after focused regression checks and an integration build. Record the verified upstream SHA and fork SHA with the build. Keep account/tab branding and registry migration tests when resolving future upstream changes. Test migrations on synthetic or copied data and inspect the settings and tab editor in light/dark themes and narrow windows.

This change integrated upstream commit `917ebd1f84eff7818e0a89bacd5e120e45b18b63` into fork base `37e4fc94d621c995c9271fb397be978b1743509a`. Recheck before publication and packaging; this recorded baseline is not a claim that upstream never advances.
