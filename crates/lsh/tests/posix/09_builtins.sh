printf '%s=%d\n' a 1 b 2
printf '%5s|%-5s|%05d|%x|%o|%c\n' ab cd 42 255 8 xyz
printf '%.2f\n' 3.14159
printf 'no newline'; echo
test 1 -eq 1 && echo eq
[ "a" = "a" ] && echo streq
[ "a" != "b" ] && echo strne
[ -z "" ] && [ -n "x" ] && echo zn
[ 2 -gt 1 -a 3 -lt 4 ] && echo andop
[ ! -e /nonexistent ] && echo notexists
[ -d / ] && [ -f /etc/passwd ] && echo files
set -- a b c; echo $#; set --; echo $#
eval 'echo evaluated $((1+1))'
cmd='echo dynamic'; eval "$cmd"
command -v cd; command -v printf >/dev/null && echo has-printf
OPTIND=1
while getopts "ab:c" opt -a -b val -c rest; do
  case $opt in
    a) echo opt-a;; b) echo "opt-b=$OPTARG";; c) echo opt-c;;
  esac
done
echo "OPTIND=$OPTIND"
cd /tmp && pwd && cd - >/dev/null && echo cd-back
umask 027; umask
read a b <<EOF
first second third
EOF
echo "$a|$b"
echo "x:y:z" | { IFS=: read p q r; echo "$p $q $r"; }
true; echo $?
