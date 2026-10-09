#!/bin/sh
# Run from a copy of this directory with GraalVM javac/native-image and a C linker.
set -eu
classifier=${1:?Select linux-x86_64-gnu or osx-aarch64}
case "$classifier" in
  linux-x86_64-gnu|osx-aarch64) ;;
  *) echo "Unsupported classifier: $classifier" >&2; exit 1 ;;
esac
cd "$(dirname "$0")"
base=https://repo.maven.apache.org/maven2
for module in bemo-api bemo-native-image; do
  curl -fsSLO "$base/dev/elide/bemo/$module/0.3.0/$module-0.3.0.jar"
done
curl -fsSLO "$base/dev/elide/bemo/bemo-native-image/0.3.0/bemo-native-image-0.3.0-$classifier.jar"
for module in nativeimage word; do
  curl -fsSLO "$base/org/graalvm/sdk/$module/25.3.4.1/$module-25.3.4.1.jar"
done
mkdir -p archive classes
(cd archive && jar xf "../bemo-native-image-0.3.0-$classifier.jar")
cp=bemo-api-0.3.0.jar:bemo-native-image-0.3.0.jar:nativeimage-25.3.4.1.jar:word-25.3.4.1.jar
javac --release 22 -cp "$cp" -d classes src/ReleaseProbe.java
native_dir="$PWD/archive/META-INF/native/$classifier"
if [ "$classifier" = linux-x86_64-gnu ]; then
  set -- -H:NativeLinkerOption=-ldl -H:NativeLinkerOption=-lpthread -H:NativeLinkerOption=-lm
else
  set --
fi
native-image --no-fallback -O2 --parallelism=8 --enable-native-access=ALL-UNNAMED \
  "-H:CLibraryPath=$native_dir" "--native-compiler-options=-I$native_dir" \
  "$@" -cp "classes:$cp" ReleaseProbe release-probe
./release-probe
