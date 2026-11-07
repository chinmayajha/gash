# Stdin of `cmd &` in a non-interactive shell: bash gives it /dev/null only
# when stdin was not redirected. An input redirection on a compound command
# (or a pipe into one, or into a function or builtin) keeps the job on that
# stdin, and the flag sticks until the script's next top-level command.
d=$(mktemp -d)
cd "$d" || exit 1
printf 'file\n' > in.txt
t() { printf '%s: [%s]\n' "$1" "$(tr '\n' '|' < o)"; }

{ /bin/cat & wait; } <<< y > o; t herestring
{ /bin/cat & wait; } <<EOF > o; t heredoc
hd
EOF
{ /bin/cat & wait; } < in.txt > o; t file
{ /bin/cat & wait; } <> in.txt > o; t read-write
exec 4< in.txt
{ /bin/cat & wait; } <&4 > o; t dup-fd0
exec 4<&-
{ /bin/cat & wait; } 3< in.txt <<< fd3 > o; t fd3-input
g() { /bin/cat & wait; }
{ g; } <<< gf > o; t group-func
f() { /bin/cat & wait; } <<< fb
f > o; t func-body-redir
if :; then /bin/cat & wait; fi <<< if > o; t if
for i in 1; do /bin/cat & wait; done <<< for > o; t for
case x in x) /bin/cat & wait;; esac <<< case > o; t case
( /bin/cat & wait ) <<< sub > o; t subshell
{ { /bin/cat; } & wait; } <<< bg-group > o; t bg-group
{ ( /bin/cat ) & wait; } <<< bg-subshell > o; t bg-subshell
{ g & wait; } <<< bg-func > o; t bg-func
{ x=$(/bin/cat & wait); echo "$x"; } <<< cmdsub > o; t cmdsub
{ eval '/bin/cat & wait'; } <<< eval > o; t eval
while read -r l; do echo "read $l"; /bin/cat & wait; done <<< $'a\nb\nc' > o; t while-read
echo p | { /bin/cat & wait; } > o; t pipe-group
echo p | while :; do /bin/cat & wait; break; done > o; t pipe-while
echo p | ( /bin/cat & wait ) > o; t pipe-subshell
echo p | g > o; t pipe-func
echo p | eval '/bin/cat & wait' > o; t pipe-eval
echo p | { /bin/cat & wait; } 2>/dev/null > o; t pipe-group-redir
/bin/cat <<< own-redir & wait
f() { /bin/cat & wait; } <<< fa; f & wait

# A compound command bash forks on its own (a pipeline stage, or `{ } &`)
# ignores its own redirections here; a pipe into it or a subshell still counts.
{ /bin/cat & wait; } <<< y | /bin/cat > o; t stage
{ /bin/cat & wait; } <<< y > o & wait; t async
{ /bin/cat & wait; } <<< y > o && : & wait; t async-and
while read -r l; do echo "read $l"; /bin/cat & wait; done <<< $'a\nb' | /bin/cat > o; t stage-while
( /bin/cat & wait ) <<< y | /bin/cat > o; t stage-subshell

# Not redirected: each job reads /dev/null, so the piped stdin is left over.
cat > n.sh <<'EOF'
/bin/cat & wait
f() { /bin/cat & wait; }
f <<< call-redir
{ :; } > /dev/null; /bin/cat & wait
{ ( /bin/cat & wait ); } <<< inner-subshell
{ /bin/cat & wait; } {fd}< in.txt
{ /bin/cat & wait; } 2>/dev/null
{ /bin/cat & wait; } 3<&-
{ { :; } < in.txt; { :; } 2>/dev/null; /bin/cat & wait; }
exec 5< in.txt; /bin/cat & wait; exec 5<&-
echo "left: $(/bin/cat)"
EOF
echo outer | "$BASH" n.sh > o; t not-redirected
echo outer | "$BASH" -c '/bin/cat & wait; echo "left: $(/bin/cat)"' > o; t plain
# The flag outlives the compound command on the same line...
echo outer | "$BASH" -c '{ :; } < in.txt; /bin/cat & wait' > o; t same-line
echo outer | "$BASH" -c 'h() { { :; } < in.txt; }; h; /bin/cat & wait' > o; t after-func
echo outer | "$BASH" -c '{ { :; } < in.txt; { /bin/cat & wait; } | /bin/cat; }' > o; t inherited-by-stage
# ...and in a -c string, on later lines too,
echo outer | "$BASH" -c '{ :; } < in.txt
/bin/cat & wait' > o; t dash-c-next-line
# but a script clears it before each command it reads.
printf '{ :; } < in.txt\n/bin/cat & wait\necho "left: $(/bin/cat)"\n' > s.sh
echo outer | "$BASH" s.sh > o; t script-next-line
# A subshell decides for itself and leaves its parent alone.
echo outer | "$BASH" -c '( : ) < in.txt; /bin/cat & wait' > o; t subshell-parent
echo outer | "$BASH" -c '{ ( /bin/cat & wait ) 3< in.txt; }' > o; t subshell-fd3

cd / && rm -rf "$d"
