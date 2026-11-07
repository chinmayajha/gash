# File tests on programs, a missing file and an empty name.
for p in /bin/ls /usr/bin/true /usr/bin/nosuch ""; do
    printf '[%s]' "${p##*/}"
    for t in -e -f -x -s -d; do if test $t "$p"; then printf ' %s' "$t"; fi; done
    echo
done
[[ -x /usr/bin/sed ]] && echo "[[ -x ]] finds sed"
[ -x "$(command -v sed)" ] && echo "sed is executable"
