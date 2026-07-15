#!/usr/bin/env bash
# Maven 域：使用隔离的官方 Maven 客户端验证 hosted、proxy 与 group。
set -euo pipefail

: "${ACCEPTANCE_BASE_URL:?缺少服务地址}"
: "${ACCEPTANCE_UPSTREAM_URL:?缺少受控上游地址}"
: "${ACCEPTANCE_RUNTIME_DIR:?缺少运行目录}"
: "${ACCEPTANCE_FIXTURE_DIR:?缺少 fixture 目录}"
: "${ACCEPTANCE_TOKEN_FILE:?缺少令牌文件}"
command -v mvn > /dev/null
command -v javac > /dev/null
command -v jar > /dev/null
mvn --version > /dev/null

readonly WORK_DIR="$ACCEPTANCE_RUNTIME_DIR/maven"
readonly LOCAL_REPO="$WORK_DIR/repository"
readonly PLUGIN_SOURCE="$ACCEPTANCE_FIXTURE_DIR/maven-plugin"
readonly PLUGIN_GROUP_PATH="com/jianartifact/acceptance/acceptance-maven-plugin/1.0.0"
readonly PLUGIN_GOAL="com.jianartifact.acceptance:acceptance-maven-plugin:1.0.0"
mkdir -p "$WORK_DIR/classes" "$LOCAL_REPO/$PLUGIN_GROUP_PATH"

fail() {
    printf '错误：Maven 验收失败：%s\n' "$1" >&2
    exit 1
}

status() {
    local expected="$1"
    shift
    local actual
    actual="$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' "$@")"
    [ "$actual" = "$expected" ] || fail "期望 HTTP $expected，实际为 $actual"
}

create_repo() {
    status 201 --header "Authorization: Bearer $(<"$ACCEPTANCE_TOKEN_FILE")" \
        --header 'Content-Type: application/json' --data-binary "$1" \
        "$ACCEPTANCE_BASE_URL/api/v1/repos"
}

build_plugin() {
    local plugin_jar="$LOCAL_REPO/$PLUGIN_GROUP_PATH/acceptance-maven-plugin-1.0.0.jar"
    local plugin_pom="$LOCAL_REPO/$PLUGIN_GROUP_PATH/acceptance-maven-plugin-1.0.0.pom"
    javac -encoding UTF-8 -cp "$MAVEN_HOME/lib/maven-plugin-api-3.9.9.jar" \
        -d "$WORK_DIR/classes" "$PLUGIN_SOURCE"/src/com/jianartifact/acceptance/*.java
    jar --create --file "$plugin_jar" \
        -C "$WORK_DIR/classes" . -C "$PLUGIN_SOURCE" META-INF
    cat > "$plugin_pom" <<'EOF'
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.jianartifact.acceptance</groupId>
  <artifactId>acceptance-maven-plugin</artifactId>
  <version>1.0.0</version>
  <packaging>maven-plugin</packaging>
</project>
EOF
    cat > "$WORK_DIR/settings.xml" <<'EOF'
<settings xmlns="http://maven.apache.org/SETTINGS/1.2.0">
  <offline>true</offline>
  <interactiveMode>false</interactiveMode>
</settings>
EOF
}

maven_goal() {
    mvn --batch-mode --errors --offline --settings "$WORK_DIR/settings.xml" \
        -Dmaven.repo.local="$LOCAL_REPO" "$@"
}

deploy() {
    local repository_url="$1" group_id="$2" artifact_id="$3" version="$4" body="$5"
    local artifact_dir="$WORK_DIR/artifacts/$artifact_id-$version"
    mkdir -p "$artifact_dir"
    printf '%s\n' "$body" > "$artifact_dir/$artifact_id.jar"
    cat > "$artifact_dir/$artifact_id.pom" <<EOF
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>$group_id</groupId><artifactId>$artifact_id</artifactId><version>$version</version>
</project>
EOF
    maven_goal -Durl="$repository_url" -DgroupId="$group_id" -DartifactId="$artifact_id" \
        -Dversion="$version" -Dfile="$artifact_dir/$artifact_id.jar" \
        -DpomFile="$artifact_dir/$artifact_id.pom" -Dacceptance.tokenFile="$ACCEPTANCE_TOKEN_FILE" \
        "$PLUGIN_GOAL:deploy-file"
}

resolve() {
    local repository_url="$1" coordinate="$2" destination="$3"
    maven_goal -DremoteRepositories="$repository_url" -Dartifact="$coordinate" \
        -Dacceptance.tokenFile="$ACCEPTANCE_TOKEN_FILE" "$PLUGIN_GOAL:dependency-get"
    local group_id artifact_id extension version relative
    IFS=: read -r group_id artifact_id extension version <<< "$coordinate"
    relative="${group_id//.//}/$artifact_id/$version/$artifact_id-$version.$extension"
    cp "$LOCAL_REPO/$relative" "$destination"
}

upstream_control() {
    curl --fail --silent --show-error --request POST --output /dev/null \
        "$ACCEPTANCE_UPSTREAM_URL/__acceptance/$1"
}

build_plugin
create_repo '{"name":"accept-maven-release","format":"maven","repo_type":"hosted","visibility":"public","overwrite_policy":"deny","version_policy":"release"}'
create_repo '{"name":"accept-maven-snapshot","format":"maven","repo_type":"hosted","visibility":"public","overwrite_policy":"deny","version_policy":"snapshot"}'
create_repo "{\"name\":\"accept-maven-proxy\",\"format\":\"maven\",\"repo_type\":\"proxy\",\"visibility\":\"public\",\"upstream_url\":\"$ACCEPTANCE_UPSTREAM_URL/maven\"}"
create_repo '{"name":"accept-maven-first","format":"maven","repo_type":"hosted","visibility":"public","overwrite_policy":"allow","version_policy":"mixed"}'
create_repo '{"name":"accept-maven-second","format":"maven","repo_type":"hosted","visibility":"public","overwrite_policy":"allow","version_policy":"mixed"}'
create_repo '{"name":"accept-maven-group","format":"maven","repo_type":"group","visibility":"public","members":["accept-maven-first","accept-maven-second"]}'

deploy "$ACCEPTANCE_BASE_URL/repository/accept-maven-release" com.example release-demo 1.0.0 release-body
resolve "$ACCEPTANCE_BASE_URL/repository/accept-maven-release" com.example:release-demo:jar:1.0.0 "$WORK_DIR/release.jar"
printf 'release-body\n' > "$WORK_DIR/release.expected"
cmp --silent "$WORK_DIR/release.expected" "$WORK_DIR/release.jar" || fail 'release deploy/get 字节不一致'

deploy "$ACCEPTANCE_BASE_URL/repository/accept-maven-snapshot" com.example snapshot-demo 1.1-SNAPSHOT snapshot-body
curl --fail --silent --show-error \
    "$ACCEPTANCE_BASE_URL/repository/accept-maven-snapshot/com/example/snapshot-demo/1.1-SNAPSHOT/maven-metadata.xml" \
    --output "$WORK_DIR/snapshot-metadata.xml"
META_FILE="$WORK_DIR/snapshot-metadata.xml" node -e '
const fs=require("node:fs"); const xml=fs.readFileSync(process.env.META_FILE,"utf8");
if (!xml.includes("<timestamp>20250101.010203</timestamp>") || !xml.includes("<buildNumber>1</buildNumber>")) process.exit(1);
' || fail 'snapshot metadata 未包含时间戳与 buildNumber'

upstream_control reset
resolve "$ACCEPTANCE_BASE_URL/repository/accept-maven-proxy" com.example:upstream-only:jar:1.0 "$WORK_DIR/proxy-first.jar"
cmp --silent "$ACCEPTANCE_FIXTURE_DIR/maven/com/example/upstream-only/1.0/upstream-only-1.0.jar" "$WORK_DIR/proxy-first.jar" || fail 'proxy 首次解析字节不一致'
node -e 'const fs=require("node:fs"); fs.rmSync(process.argv[1],{force:true})' \
    "$LOCAL_REPO/com/example/upstream-only/1.0/upstream-only-1.0.jar"
upstream_control disable
resolve "$ACCEPTANCE_BASE_URL/repository/accept-maven-proxy" com.example:upstream-only:jar:1.0 "$WORK_DIR/proxy-offline.jar"
cmp --silent "$WORK_DIR/proxy-first.jar" "$WORK_DIR/proxy-offline.jar" || fail 'proxy 断源缓存解析失败'
upstream_control enable

deploy "$ACCEPTANCE_BASE_URL/repository/accept-maven-first" com.example group-conflict 1.0 first-member
deploy "$ACCEPTANCE_BASE_URL/repository/accept-maven-second" com.example group-conflict 1.0 second-member
deploy "$ACCEPTANCE_BASE_URL/repository/accept-maven-second" com.example group-only 2.0 second-only
resolve "$ACCEPTANCE_BASE_URL/repository/accept-maven-group" com.example:group-conflict:jar:1.0 "$WORK_DIR/group-conflict.jar"
printf 'first-member\n' > "$WORK_DIR/group-conflict.expected"
cmp --silent "$WORK_DIR/group-conflict.expected" "$WORK_DIR/group-conflict.jar" || fail 'group 冲突未按前序成员解析'
resolve "$ACCEPTANCE_BASE_URL/repository/accept-maven-group" com.example:group-only:jar:2.0 "$WORK_DIR/group-only.jar"
printf 'second-only\n' > "$WORK_DIR/group-only.expected"
cmp --silent "$WORK_DIR/group-only.expected" "$WORK_DIR/group-only.jar" || fail 'group 未解析后序独有构件'

printf 'Maven 官方客户端验收通过\n'
