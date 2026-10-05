# Printed by every interactive shell in the waserver container (sourced from
# /etc/bash.bashrc, and via $ENV for sh), so `docker exec -it … bash` and a
# platform's Terminal tab start with the state of the shares and the commands
# that matter. Nothing here runs for non-interactive shells.
case $- in *i*) ;; *) return 0 2>/dev/null || exit 0 ;; esac

share="${SHARE_ID:-default}"
echo "Wispers Access host. Shares and their state live on /data."
echo
waserver status 2>/dev/null || echo "(waserver status failed; is the server up?)"
config="${XDG_CONFIG_HOME:-$HOME/.config}/waserver/shares/$share/share.toml"
if [ -L "$config" ]; then
  edit_line="  (config is mounted from $(readlink "$config"); edit it there, then: waserver reload $share)"
else
  edit_line="  waserver edit $share                              add or change apps, then reload"
fi
cat <<EOT

  waserver status $share                            guests and apps in detail
$edit_line
  waserver invite $share "Alice's phone" alice@example.com
  waserver revoke $share <number>                   cut a guest off
  waserver logs -f                                   follow the server logs

Editors: nano (default), vim, mg. \`select-editor\` or the EDITOR variable picks.
EOT
