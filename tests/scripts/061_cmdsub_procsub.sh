# command substitution, part 2: functions, word splitting of results, process substitution, backtick escapes
x=hello
f() { echo "func out"; }
echo "c1: $(f)"
g=$(f; f); echo "c2: $(echo "$g" | wc -l)"
k="$(echo "a     b")"; echo "c3: $k"
k2=$(echo "a     b"); echo "c4: $k2"
echo "c5: $(echo $((2+3)))"
echo "c6: $(printf '%s' "$(printf 'x\n\n')")|"
local_test() { local r=$(exit 5); echo "c7: $?"; }
local_test
echo "c8: $(echo "nested \"quotes\" here")"
while read -r l; do echo "c9: $l"; done < <(printf 'p1\np2\n')
cat <(echo procsub) | sed 's/^/c10: /'
mapfile -t ps < <(printf 'x\ny\n'); echo "c11: ${#ps[@]}"
echo "c12: $(cat <(echo a) <(echo b) | tr '\n' ' ')"
cnt=0; while read -r _; do ((cnt++)); done < <(printf '1\n2\n3\n'); echo "c13: count survives $cnt"
echo "c14: `echo a; echo b`"
echo "c15: $(echo $(echo "a   b"))"
echo "c16: $(echo "$(echo "a   b")")"
u=$(printf '%s' 'no newline'); echo "c17: [$u]"
e=$(echo); echo "c18: [$e] ${#e}"
o=$(echo -n); echo "c19: [$o]"
sub=$(x=changed; echo $x); echo "c20: $sub $x"
p=$(cd / && echo "in root"); echo "c21: $p"
echo "c22: $(echo 1)$(echo 2)$(echo 3)"
arr=($(echo "w1 w2 w3")); echo "c23: ${#arr[@]}"
echo "c24: $(printf '%s\n' "a b" | { read -r q1 q2; echo "$q2$q1"; })"
bt=`printf '%s' "\`echo deep\`"`; echo "c25: $bt"
echo "c26: $(echo \$x) `echo \\$x`"
echo "c27: $(true && echo and) $(false || echo or)"
