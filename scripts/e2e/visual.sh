# Scenario: visual QA in light and dark — the Sites page, the site dialog and a connected tab with
# a file open and the log open, at 1280×800. The colour mode is changed through the UI
# (ThemeToggle → Appearance → Dark) and must survive a restart. Compare the shots with
# ../zap/docs/zap-screenshot-{light,dark}.png (docs/e2e.md).
. "$ROOT/scripts/e2e/lib.sh"

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "docker sftp" sftp 2222 explicit /home/gem/samples
seed_site 01e2e000-0000-7000-8000-000000000002 "vsftpd plain" ftp 2121
seed_site 01e2e000-0000-7000-8000-000000000003 "proftpd ftps" ftps 2321 explicit
trust_sftp
start_gem
hover 640 700
shot 01-sites-light
assert_tone "light: Sites page background is light" 01-sites-light 640 500 light
click 1221 63                 # New site
sleep 1
hover 1100 700
shot 02-site-dialog-light
click 765 618                 # Cancel
sleep 0.5

connect_site 0                # opens in /home/gem/samples (the site's initial path)
preview index.php
hover 640 700
shot 03-connected-viewer-log-light
assert_tone "light: editor canvas is light" 03-connected-viewer-log-light 1000 450 light

# ---- ThemeToggle → Appearance → Dark ----
click 1041 18                 # the palette button of the title bar (connected layout)
sleep 0.8
shot 04-appearance-dialog-light
click 713 365                 # Dark
sleep 0.4
click 762 498                 # Done
sleep 0.8
hover 640 700
shot 05-connected-viewer-log-dark
assert_tone "dark: editor canvas is dark" 05-connected-viewer-log-dark 1000 450 dark
assert_tone "dark: log panel is dark" 05-connected-viewer-log-dark 900 700 dark

new_sites_tab
hover 640 700
shot 06-sites-dark
assert_tone "dark: Sites page background is dark" 06-sites-dark 640 500 dark
click 1221 95                 # New site (below the tab strip)
sleep 1
hover 1100 700
shot 07-site-dialog-dark
click 765 618                 # Cancel
sleep 0.5

# ---- the colour mode survives a restart ----
stop_gem
sleep 1
start_gem
hover 640 700
shot 08-sites-dark-after-restart
assert_tone "dark mode kept after a restart" 08-sites-dark-after-restart 640 500 dark
stop_gem
