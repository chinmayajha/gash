# common string-manipulation idioms
s="   hello world   "
trimmed="${s#"${s%%[![:space:]]*}"}"
trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
echo "c1: [$trimmed]"
ltrim="${s#"${s%%[! ]*}"}"; echo "c2: [$ltrim]"
path="a:b:c:d"
IFS=: read -ra parts <<< "$path"
echo "c3: ${#parts[@]} ${parts[2]}"
for p in ${path//:/ }; do echo "c4: $p"; done
old_ifs=$IFS; IFS=:; set -- $path; IFS=$old_ifs; echo "c5: $# $3"
arr=(x y z)
joined=$(IFS=,; echo "${arr[*]}"); echo "c6: $joined"
join_by() { local d=$1; shift; local f=$1; shift; printf '%s' "$f" "${@/#/$d}"; }
echo "c7: $(join_by ' - ' a b c)"
file="/home/user/docs/report.final.pdf"
b=${file##*/}
echo "c8: base=$b dir=${file%/*} ext=${file##*.} stem=${b%.*}"
echo "c9: ${file^^}"
name="John"; echo "c10: ${name,,} ${name^^}"
printf 'c11: [%-10s][%10s][%05d]\n' "$name" "$name" 42
line=$(printf '%*s' 20 ''); echo "c12: [${line// /=}]"
str="mississippi"
echo "c13: len=${#str}"
only_s="${str//[^s]/}"; echo "c14: s count=${#only_s}"
rev=""; for ((i=${#str}-1; i>=0; i--)); do rev+="${str:i:1}"; done; echo "c15: $rev"
[[ $str == *ssi* ]] && echo "c16: contains ssi"
[[ $str == miss* ]] && echo "c17: startswith miss"
[[ $str == *pi ]] && echo "c18: endswith pi"
echo "c19: ${str/ss/SS} ${str//ss/SS}"
first_char=${str:0:1}; last_char=${str: -1}; echo "c20: $first_char $last_char"
echo "c21: ${str^}"
words="the quick brown fox"
set -- $words; echo "c22: $# words, last=${!#}"
count=0; for w in $words; do (( count++ )); done; echo "c23: $count"
csv="name,age,city"
IFS=, read -r f1 f2 f3 <<< "$csv"; echo "c24: $f3|$f2|$f1"
repeat() { local out="" k; for ((k=0; k<$2; k++)); do out+=$1; done; echo "$out"; }
echo "c25: $(repeat ab 3)"
num=7; printf -v padded '%03d' "$num"; echo "c26: $padded"
kv="key=value=more"; echo "c27: k=${kv%%=*} v=${kv#*=}"
url="https://example.com:8080/path/to?q=1"
proto=${url%%://*}; rest=${url#*://}; host=${rest%%[:/]*}; echo "c28: $proto $host"
ver="1.22.3"; IFS=. read -r major minor patch <<< "$ver"; echo "c29: $major $minor $patch"
(( major == 1 && minor > 20 )) && echo "c30: version ok"
lower="abc"; [[ $lower =~ ^[a-z]+$ ]] && echo "c31: all lower"
r=$RANDOM; (( r >= 0 && r < 32768 )) && echo "c32: random in range"
d=$(( RANDOM % 6 + 1 )); (( d >= 1 && d <= 6 )) && echo "c33: dice in range"
s2="a-b_c d"; echo "c34: ${s2//[-_ ]/.}"
text=$'line1\nline2\nline3'
echo "c35: $(echo "$text" | wc -l) lines, first=${text%%$'\n'*}, last=${text##*$'\n'}"
upper_first() { local w out=(); for w in "$@"; do out+=("${w^}"); done; echo "${out[*]}"; }
echo "c36: $(upper_first hello big world)"
is_int() { [[ $1 =~ ^-?[0-9]+$ ]]; }
for v in 42 -7 3.5 abc ""; do is_int "$v" && echo "c37: [$v] int" || echo "c37: [$v] not int"; done
snake="HelloWorldFooBar"; out=""
for ((i=0; i<${#snake}; i++)); do ch=${snake:i:1}; if [[ $ch == [A-Z] ]]; then (( i > 0 )) && out+="_"; out+=${ch,}; else out+=$ch; fi; done
echo "c38: $out"
camel="hello_world_foo"; IFS=_ read -ra seg <<< "$camel"; res=${seg[0]}; for sg in "${seg[@]:1}"; do res+=${sg^}; done; echo "c39: $res"
pad_center() { local t=$1 w=$2; local l=$(( (w - ${#t}) / 2 )); local r=$(( w - ${#t} - l )); printf '%*s%s%*s' $l '' "$t" $r ''; }
echo "c40: [$(pad_center hi 8)]"
dup="a,b,a,c,b"; uniq_out=$(tr ',' '\n' <<< "$dup" | sort -u | tr '\n' ' '); echo "c41: $uniq_out"
sentence="The Cat"; [[ ${sentence,,} == "the cat" ]] && echo "c42: case-insensitive compare"
hex=$(printf '%02x' 255 16 1); echo "c43: $hex"
ord=$(printf '%d' "'a"); chr=$(printf "\\$(printf '%03o' 66)"); echo "c44: $ord $chr"
indent() { local l; while IFS= read -r l; do echo "    $l"; done; }
printf 'x\ny\n' | indent | sed 's/^/c45:/'
