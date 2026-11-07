# MSYS programs appending to one file at the same time keep all of their output, through one
wc() { command wc "$@" | tr -d ' '; }
# redirection and through separate ones, as with Git Bash's O_APPEND.
t=$(mktemp -d)
cd "$t" || exit 1

for i in $(seq 1 30); do /bin/echo "job $i" >> m & done
wait
echo "separate redirections: $(wc -l < m) lines, $(sort -u m | wc -l) distinct"

for i in $(seq 1 30); do { /bin/echo "shared $i"; } & done >> n
wait
echo "one redirection: $(wc -l < n) lines, $(sort -u n | wc -l) distinct"

for i in $(seq 1 20); do { echo "builtin $i"; /bin/echo "msys $i"; } >> k & done
wait
echo "builtins and MSYS programs: $(wc -l < k) lines, $(sort -u k | wc -l) distinct"

# Nothing keeps the file open afterwards: it can be removed and created again.
/bin/echo old >> o
rm o
echo new > o
cat o

cd / && rm -rf "$t"
