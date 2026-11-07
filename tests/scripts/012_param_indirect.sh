# ${!ref} indirection, ${!prefix@}, ${x@Q} and other @ operators, namerefs
target="the value"
ref=target
echo "c1: ${!ref}"
arr=(x y z)
ref2='arr[1]'
echo "c2: ${!ref2}"
ref3='arr[@]'
echo "c3: ${!ref3}"
myvar_a=1 myvar_b=2 myvar_c=3 other=4
echo "c4: ${!myvar_@}"
echo "c5: ${!myvar_*}"
for n in "${!myvar_@}"; do echo "c6: $n=${!n}"; done
s="it's a 'test'"
echo "c7: ${s@Q}"
t="plain"
echo "c8: ${t@Q}"
u=$'tab\there'
echo "c9: ${u@Q}"
e=""
echo "c10: ${e@Q}"
a=("a b" c)
echo "c11: ${a[@]@Q}"
echo "c12: ${t@U} ${t@u} ${s@L}"
declare -i num=5
echo "c13: ${num@a} ${a@a} ${t@a}"
declare -A h=([k]=v)
echo "c14: ${h@a}"
esc='a\tb'
echo "c15: ${esc@E}" | cat -vet
echo "c16: ${t@A}"
echo "c17: ${num@A}"
declare -n nr=target
echo "c18: $nr"
nr="changed"
echo "c19: $target"
setter() { local -n out=$1; out="set by setter"; }
setter result
echo "c20: $result"
appender() { local -n arr_ref=$1; arr_ref+=("$2"); }
list=(a)
appender list b; appender list "c d"
echo "c21: ${#list[@]} ${list[2]}"
set -- p1 p2 p3
n=2
echo "c22: ${!n} ${!#}"
idx_name=myvar_b
echo "c23: $(( ${!idx_name} * 10 ))"
unset_ref=does_not_exist
echo "c24: [${!unset_ref}] [${!unset_ref:-fallback}]"
q="\$dollar \"dq\" back\\slash"
echo "c25: ${q@Q}"
declare -n alias_arr=arr
echo "c26: ${alias_arr[2]} ${#alias_arr[@]}"
sum_ref() { local -n nums=$1; local total=0 v; for v in "${nums[@]}"; do (( total += v )); done; echo "$total"; }
values=(1 2 3 4)
echo "c27: $(sum_ref values)"
echo "c28: ${!myvar_a*}"
k=t
echo "c29: ${!k^^} ${#k}"
