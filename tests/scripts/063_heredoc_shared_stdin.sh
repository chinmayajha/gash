# Here-doc and here-string input that a child process doesn't read stays there
# for the next reader (issue #2: it was handed whole to the first child, so a
# later `cat` got nothing)
d=$(mktemp -d)
cd "$d" || exit 1

# a script that runs a program before reading stdin
printf '#!/usr/bin/env bash\n/usr/bin/true\ncat\n' > repro
chmod +x repro
echo "script: [$(./repro <<< data)]"
echo "script heredoc: [$(./repro <<EOF
line one
line two
EOF
)]"

# a function, with a command substitution running the program
f() { x=$(/usr/bin/true); cat; }
echo "func cmdsub: [$(f <<< data)]"
g() { /usr/bin/true; /bin/cat; }
echo "func msys cat: [$(g <<< data)]"

# the wrapper pattern from the report: resolve a path, then exec the reader
printf '#!/usr/bin/env bash\np=$(/usr/bin/cygpath -w /)\nexec /bin/cat\n' > launcher
chmod +x launcher
./launcher <<'EOF'
echo remote script
exit 0
EOF

# readers that take part of the input leave the rest
{ read -r a; /usr/bin/true; read -r b; /bin/cat; } <<EOF
1
2
3
4
EOF
echo "a=$a b=$b"
{ /usr/bin/head -n 1; echo "--"; cat; } <<EOF
first
second
third
EOF
{ read -r x; /usr/bin/head -n 1; read -r y; echo "x=$x y=$y"; cat; } <<< $'p\nq\nr\ns'

# a loop over here-doc lines that runs programs in its body
while read -r line; do
  /usr/bin/true
  echo "got $line"
done <<EOF
alpha
beta
gamma
EOF

# expansions in the body are still done once, at redirection time
n=5
h() { /usr/bin/true; cat; }
h <<EOF
n=$n $(echo sub)
EOF

# input bigger than a pipe buffer goes in a file, which head can seek back in
big=$(printf 'line %d\n' {1..10000})
{ /usr/bin/true; /usr/bin/wc -l; } <<< "$big"
{ /usr/bin/head -n 1; /usr/bin/wc -l; } <<< "$big"
{ read -r l; /usr/bin/head -n 2; read -r l2; echo "l=$l l2=$l2"; /usr/bin/wc -l; } <<< "$big"
f <<< "$big" | /usr/bin/wc -c

# each command gets its own copy
for i in 1 2; do /usr/bin/true; cat; done <<< once
cat <<< again; /bin/cat <<< again

cd / && rm -rf "$d"
