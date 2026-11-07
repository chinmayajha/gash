# MSYS programs given /dev/stdin, /dev/fd/N or /proc/self/fd/N as a file name.
# With a here-string or here-doc on stdin they failed with "No such file or
# directory": the data came through a pipe, which MSYS can only reopen when it
# created the pipe itself. Such programs now get the input as a file.
# Not covered: a pipe between gash and an MSYS program has the same problem and
# can't be fixed from outside MSYS, and /dev/stdout targets that are pipes are
# skipped since the harness captures stdout through one. Partial reads by name
# (`head -c2 /dev/stdin; cat`) also differ: a reopened file can't tell gash how
# much was read.
d=$(mktemp -d)
cd "$d" || exit 1
printf 'one\ntwo\n' > in.txt
v=expanded
echo "== here-string"
/bin/cat /dev/stdin <<< x; echo "rc=$?"
/bin/cat /dev/fd/0 <<< y; echo "rc=$?"
/bin/cat /proc/self/fd/0 <<< z; echo "rc=$?"
/usr/bin/head -n1 /dev/stdin <<< "$v"
/usr/bin/wc -c /dev/stdin <<< abcd
/usr/bin/diff /dev/stdin in.txt <<< one; echo "rc=$?"
/usr/bin/cmp /dev/stdin in.txt <<< "$(printf 'one\ntwo')"; echo "rc=$?"
echo "== here-doc"
/bin/cat /dev/stdin <<EOF
a $v
b
EOF
/usr/bin/sort -r /dev/fd/0 <<'EOF'
1 $v
2
3
EOF
/bin/cat /dev/stdin <<-EOF
	tabs stripped
	EOF
echo "== here-doc on a group and a subshell"
{ /bin/cat /dev/stdin; } <<< group
( /bin/cat /dev/stdin ) <<< subshell
f() { /bin/cat /dev/stdin; }
f <<< function
echo "== file"
/bin/cat /dev/stdin < in.txt
/bin/cat /dev/fd/0 < in.txt
/usr/bin/tail -n1 /proc/self/fd/0 < in.txt
exec 3< in.txt
/bin/cat /dev/stdin <&3
exec 3<&-
echo "== /dev/stdout and /dev/stderr into files"
/bin/cp in.txt /dev/stdout > o.txt; echo "rc=$? [$(tr '\n' '|' < o.txt)]"
/bin/cp in.txt /dev/fd/1 > o.txt; echo "rc=$? [$(tr '\n' '|' < o.txt)]"
/bin/cp in.txt /dev/stderr 2> e.txt; echo "rc=$? [$(tr '\n' '|' < e.txt)]"
/bin/cp in.txt /dev/fd/2 2> e.txt; echo "rc=$? [$(tr '\n' '|' < e.txt)]"
/bin/cat /dev/stdin <<< both > o.txt; echo "rc=$? [$(tr '\n' '|' < o.txt)]"
/bin/cat /dev/stdin <<< app >> o.txt; echo "rc=$? [$(tr '\n' '|' < o.txt)]"
echo "== later readers of the same here-doc"
{ /bin/cat /dev/stdin; read -r x; echo "[$x]"; } <<< x
{ /bin/cat /dev/stdin; /bin/cat /dev/stdin; echo end; } <<< once
{ read -r a; /bin/cat /dev/stdin; } <<< $'1\n2'
{ /usr/bin/head -n1 /dev/stdin; cat; } <<< $'1\n2\n3'
{ /usr/bin/head -n1 /dev/stdin; /usr/bin/head -n1; echo "[$(cat)]"; } <<< $'1\n2\n3'
/bin/sh -c 'cat /dev/stdin' <<< via-sh
/usr/bin/awk '{ print "awk", $0 }' /dev/stdin <<< line
echo "== input bigger than a pipe buffer is a file, reopened from its start"
big=$(printf 'line %d\n' {1..10000})
{ /usr/bin/head -n1 /dev/stdin; /usr/bin/wc -l; } <<< "$big"
{ read -r l; /usr/bin/wc -l /dev/stdin; /usr/bin/wc -l; } <<< "$big"
echo "== builtins and native redirections still see the data"
read -r line < /dev/stdin <<< builtin; echo "$line"
cat /dev/stdin <<< plain-cat
cd / && rm -rf "$d"
