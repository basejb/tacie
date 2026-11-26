#!/bin/sh
# 브랜치 이름 표의 기대값을 진짜 git으로 한 번 만드는 스크립트예요(테스트는 git 없이 fixture만 읽어요).
# 사용: sh make_branch_names.sh > branch_names.json
# 기대값은 `git check-ref-format "refs/heads/$name"`의 종료 코드이고, 앞 검사(빈 이름과 `-`로 시작)는 따로 false예요.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
python3 -I - <<'PY'
import json, subprocess
long255 = "a" * 255
names = [
    "main", "feat/x", "a/b/c", "x", "release-1.2", "fix_bug", "HEAD", "refs/heads/x",
    "", "-x", "-", "--upload-pack=x",
    "@", "@{x}", "a@{b", "@{-1}", "a@b", "@a", "a@",
    "a..b", "..", ".", "a.", "x.", "x..", ".a", "a/.b", "a/./b", "a/../b",
    "a.lock", "a.lock.b", "a/b.lock/c", "a/b.lock", ".lock", "lock",
    "a//b", "/a", "a/", "/",
    "a b", " a", "a ", "a\tb", "a\nb", "a\x01b", "a\x1fb", "a\x7fb", "a\x00b".replace("\x00", ""),
    "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b", "a]b", "a{b", "a}b",
    "한글/브랜치", "🐙", "%41", "a#b", "a+b", "a=b", "a,b", "a;b", "a!b", "a$b", "a&b", "a'b", 'a"b',
    "a(b)", "a<b>", "a|b", "a`b", "a-b", "a_b", "ÀÉ", "a b", "a​b",
    long255,
]
out = []
for n in names:
    if n == "" or n.startswith("-"):
        ok = False
    else:
        ok = subprocess.run(["git", "check-ref-format", "refs/heads/" + n],
                            capture_output=True).returncode == 0
    out.append([n, ok])
print("[")
print(",\n".join("  " + json.dumps(e, ensure_ascii=False) for e in out))
print("]")
PY
