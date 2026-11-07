# As with Cygwin, a path names foo.exe when only that exists, so file tests find programs under
# the names `command -v` prints.
for p in /bin/ls /usr/bin/ls.exe "$(command -v git)" /usr/bin/nosuch ""; do
    printf '[%s]' "${p##*/}"
    for t in -e -f -x -s -d; do if test $t "$p"; then printf ' %s' "$t"; fi; done
    echo
done
[[ -x /usr/bin/sed ]] && echo "[[ -x ]] finds sed"
[ -x "$(command -v git)" ] && echo "git is executable"
