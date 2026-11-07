# subshell ( ) isolation vs { } groups
x=outer
(x=inner; echo "c1: in subshell $x")
echo "c2: after $x"
{ x=group; echo "c3: in group $x"; }
echo "c4: after group $x"
(exit 5); echo "c5: $?"
(echo "c6: before exit"; exit 2; echo "never"); echo "c6: status $?"
tmp=$(mktemp -d) || exit 1
cd "$tmp" || exit 1
mkdir sub
(cd sub && [ ! -d sub ] && echo "c7: inside sub")
[ -d sub ] && echo "c8: still in parent"
cd .. && rm -rf "$tmp"
f() { echo "c9: func visible in subshell"; }
(f)
(g() { echo inner; }); type -t g >/dev/null || echo "c10: func defined in subshell gone"
( set -- a b; echo "c11: $#" ); echo "c12: $#"
[[ $$ == $BASHPID ]] && echo "c13: main pid same"
( [[ $$ == $BASHPID ]] || echo "c14: subshell BASHPID differs" )
( [[ $$ == "$(echo $$)" ]] && echo "c15: \$\$ same in subshell" )
echo "c16: $BASH_SUBSHELL $(echo $BASH_SUBSHELL) $( (echo $BASH_SUBSHELL) )"
( (echo "c17: nested $BASH_SUBSHELL") )
v=1; ( v=2; ( v=3; echo "c18: $v" ); echo "c19: $v" ); echo "c20: $v"
{ echo "c21: a"; echo "c21: b"; } | sed 's/c21/C21/'
r=$( { echo g1; echo g2; } | wc -l ); echo "c22: $r"
{ false; }; echo "c23: $?"
( true; false ); echo "c24: $?"
(exit 300); echo "c25: $?"
{ x=1; y=2; }; echo "c26: $x $y"
( unset x; echo "c27: [${x-unset}]" ); echo "c28: [$x]"
( shopt -s extglob; echo "c29: set in subshell" ); shopt -q extglob && echo yes || echo "c30: extglob not leaked"
( set -e; false; echo never ); echo "c31: $?"
out=$( exit 9 ); echo "c32: $?"
( trap 'echo "c33: subshell exit trap"' EXIT; echo "c33: body" )
{ echo "c34: one-line group"; }
(
  echo "c35: multiline subshell"
)
{
  echo "c36: multiline group"
}
( exit ); echo "c37: bare exit status $?"
false; ( exit ); echo "c38: bare exit inherits $?"
( false; exit ); echo "c39: $?"
arr=(1 2); ( arr+=(3); echo "c40: ${#arr[@]}" ); echo "c41: ${#arr[@]}"
declare -A h=([a]=1); ( h[b]=2; echo "c42: ${#h[@]}" ); echo "c43: ${#h[@]}"
( IFS=,; set -- x y; echo "c44: $*" ); set -- x y; echo "c45: $*"
{ echo "c46: group status"; (exit 4); }; echo "c46: $?"
( echo "c47: subshell output" ) > /dev/null; echo "c47: redirected away"
res=$( (echo inner; exit 3); echo "after $?" ); echo "c48: $res"
( ( ( echo "c49: triple nesting" ) ) )
z=$( { echo a; echo b; } | sort -r | head -n 1 ); echo "c50: $z"
