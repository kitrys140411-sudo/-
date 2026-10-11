echo $(echo inner)
echo "$(echo "quoted $(echo nested)")"
echo `echo backquote`
x=$(printf 'a\nb\n\n\n'); echo "[$x]"
y=$(false); echo "status $?"
z=$(exit 7); echo "status $?"
echo $(echo a; echo b)
echo "$(echo a; echo b)"
n=$(echo 5); echo $((n * 2))
echo "$(cd /tmp && pwd)"; pwd >/dev/null
v=$(echo "  spaced  "); echo "[$v]"
echo $( case x in x) echo case-in-subst;; esac )
