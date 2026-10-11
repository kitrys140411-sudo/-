a=1 b=2; echo $a $b
c="$a$b"; echo "$c"
unset a; echo "[${a}]" "[${a-unset}]" "[${a:-default}]"
e=; echo "[${e-x}]" "[${e:-x}]" "[${e+set}]" "[${e:+nonempty}]"
echo "${f=assigned}" "$f"
s=hello; echo ${#s} ${#nothing}
p=/usr/local/lib/libfoo.so.1
echo ${p#*/} ${p##*/} ${p%.*} ${p%%.*}
echo "${p#"/usr"}"
set -- one "two three" four
echo $# "$1" "$2" "$3"
for x in "$@"; do echo "[$x]"; done
for x in $@; do echo "<$x>"; done
echo "$*"
IFS=:; echo "$*"; IFS=' 	
'
shift; echo $# "$1"
shift 2; echo $#
f() { echo "fn: $# $1"; }; f a b
echo ${unset_var:-$((2*21))}
readonly ro=1; (ro=2) 2>/dev/null || echo "readonly ok"
x=outer; f2() { local x=inner; echo $x; }; f2; echo $x
