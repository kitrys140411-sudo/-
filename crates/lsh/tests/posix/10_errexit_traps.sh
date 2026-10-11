trap 'echo exit-trap' EXIT
(set -e; false; echo not-reached); echo "sub status $?"
(set -e; false || true; echo continues)
(set -e; if false; then :; fi; echo if-ok)
(set -e; ! true; echo neg-ok)
(set -u; echo "${undefined}"; echo no) 2>/dev/null; echo "nounset status $?"
trap 'echo got-usr1' USR1
kill -USR1 $$
echo after-signal
f() { trap 'echo inner-trap' USR2; kill -USR2 $$; }; f
echo end
