if true; then echo t1; fi
if false; then echo no; elif true; then echo t2; else echo no; fi
if false; then :; else echo t3; fi
i=0; while [ $i -lt 3 ]; do echo w$i; i=$((i+1)); done
i=0; until [ $i -ge 2 ]; do echo u$i; i=$((i+1)); done
for x in a b c; do echo f$x; done
for x in 1 2 3 4 5; do
  [ $x = 2 ] && continue
  [ $x = 4 ] && break
  echo loop$x
done
for i in 1 2; do for j in a b c; do [ $j = b ] && continue 2; echo $i$j; done; done
for i in 1 2; do for j in a b; do [ $i = 2 ] && break 2; echo n$i$j; done; done
case foo.c in
  *.h) echo header;;
  *.c|*.cc) echo source;;
  *) echo other;;
esac
case "x y" in "x y") echo quoted-match;; esac
case abc in a?c) echo qmark;; esac
case Z in [a-z]) echo lower;; [A-Z]) echo upper;; esac
case none in a) echo a;; esac; echo "case status $?"
x=3; case $x in 1|2) echo small;; 3) echo three ;; esac
while false; do :; done; echo "while status $?"
i=0; while :; do i=$((i+1)); if [ $i -gt 2 ]; then break; fi; done; echo "i=$i"
