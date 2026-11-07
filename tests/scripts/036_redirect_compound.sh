wc() { command wc "$@" | tr -d ' '; }
# redirections on compound commands, >| and noclobber
tmp=$(mktemp -d) || exit 1
cd "$tmp" || exit 1
{ echo g1; echo g2; } > group.txt
echo "c1: $(tr '\n' ' ' < group.txt)"
printf 'l1\nl2\nl3\n' > lines.txt
n=0
while read -r l; do n=$((n+1)); echo "c2: $n $l"; done < lines.txt
echo "c3: n after loop = $n"
for i in 1 2 3; do echo "item$i"; done > for.txt
echo "c4: $(wc -l < for.txt)"
if true; then echo in-if; fi > if.txt
echo "c5: $(cat if.txt)"
( echo sub1; echo sub2 ) > sub.txt
echo "c6: $(tail -n 1 sub.txt)"
case x in x) echo in-case;; esac > case.txt
echo "c7: $(cat case.txt)"
f() { echo in-func; }
f > func.txt; echo "c8: $(cat func.txt)"
g() { echo body-redirect; } > g.txt
g; echo "c9: $(cat g.txt)"
g; g; echo "c10: $(wc -l < g.txt)"
{ read -r first; read -r second; } < lines.txt; echo "c11: $first $second"
while read -r l; do echo "$l"; done < lines.txt > copy.txt; echo "c12: $(head -n 1 copy.txt)"
echo original > nc.txt
set -C
( echo clobber > nc.txt ) 2>/dev/null; echo "c13: noclobber status $?"
echo "c14: $(cat nc.txt)"
echo forced >| nc.txt; echo "c15: $(cat nc.txt)"
echo appended >> nc.txt; echo "c16: $(wc -l < nc.txt)"
echo newfile > brandnew.txt; echo "c17: new file ok $?"
echo x > /dev/null; echo "c18: /dev/null ok $?"
set +C
echo clobbered > nc.txt; echo "c19: $(cat nc.txt)"
{ echo out; echo err >&2; } 2>/dev/null | sed 's/^/c20: /'
until [ -s u.txt ]; do echo filled; done > u.txt; echo "c21: $(cat u.txt)"
{ cat; } < lines.txt | head -n 2 | sed 's/^/c22: /'
x=$(while read -r l; do echo "${l^^}"; done < lines.txt | tr '\n' ,); echo "c23: $x"
set -o noclobber
( : > nc.txt ) 2>/dev/null; echo "c24: noclobber via set -o $?"
( echo x > nc.txt; echo "c25: subshell continues after failed redirect" ) 2>/dev/null; echo "c25: status $?"
set +o noclobber
{ echo a; echo b; } >> group.txt; echo "c26: $(wc -l < group.txt)"
{ while read -r l; do echo "w:$l"; done; } < lines.txt > w.txt; echo "c27: $(tail -n 1 w.txt)"
for i in 1 2; do echo "loop$i"; done | { read -r a; read -r b; echo "c28: $a $b"; }
if [ -f lines.txt ]; then cat; fi < lines.txt | sed -n '2p' | sed 's/^/c29: /'
total=0; while read -r l; do (( total += ${#l} )); done < lines.txt; echo "c30: $total"
{ echo "c31: stderr of group" >&2; echo "c31: stdout hidden"; } 2>&1 >/dev/null | cat
cd .. && rm -rf "$tmp"
