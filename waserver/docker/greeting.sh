# Printed by every interactive shell in the waserver container (sourced from
# /etc/bash.bashrc, and via $ENV for sh), so `docker exec -it … bash` and a
# platform's Terminal tab start with the state of the shares and the commands
# that matter. Nothing here runs for non-interactive shells.
case $- in *i*) ;; *) return 0 2>/dev/null || exit 0 ;; esac

share="${SHARE_ID:-default}"
echo "Wispers Access host. Shares and their state live on /data."
echo
waserver status 2>/dev/null || echo "(waserver status failed; is the server up?)"
cat <<EOT

  waserver status $share                            guests and apps in detail
  waserver edit $share                              add or change apps, then reload
  waserver invite $share "Alice's phone" alice@example.com
  waserver revoke $share <number>                   cut a guest off
  waserver logs -f                                   follow the server logs

Editors: nano (default), vim, mg. \`select-editor\` or the EDITOR variable picks.
EOT
