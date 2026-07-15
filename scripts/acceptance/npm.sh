#!/usr/bin/env bash
# npm 域：使用隔离的官方 npm 客户端验证 hosted、proxy 与 group。
set -euo pipefail

: "${ACCEPTANCE_BASE_URL:?缺少服务地址}"
: "${ACCEPTANCE_UPSTREAM_URL:?缺少受控上游地址}"
: "${ACCEPTANCE_RUNTIME_DIR:?缺少运行目录}"
: "${ACCEPTANCE_TOKEN_FILE:?缺少令牌文件}"
command -v npm > /dev/null
command -v node > /dev/null
npm --version > /dev/null

readonly WORK_DIR="$ACCEPTANCE_RUNTIME_DIR/npm"
readonly HOME_DIR="$WORK_DIR/home"
readonly CACHE_DIR="$WORK_DIR/cache"
mkdir -p "$HOME_DIR" "$CACHE_DIR" "$WORK_DIR/packages" "$WORK_DIR/install"
umask 077

fail() {
    printf '错误：npm 验收失败：%s\n' "$1" >&2
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

registry_url() {
    printf '%s/repository/%s/\n' "$ACCEPTANCE_BASE_URL" "$1"
}

npm_config() {
    local repository="$1" target="$2" token authority path
    token="$(<"$ACCEPTANCE_TOKEN_FILE")"
    authority="${ACCEPTANCE_BASE_URL#http://}"
    path="/repository/$repository/"
    cat > "$target" <<EOF
registry=$(registry_url "$repository")
//$authority$path:_authToken=$token
always-auth=true
audit=false
fund=false
update-notifier=false
fetch-retries=0
fetch-timeout=10000
EOF
}

npm_isolated() {
    local config="$1"
    shift
    HOME="$HOME_DIR" npm_config_cache="$CACHE_DIR" npm_config_userconfig="$config" \
        npm --silent --ignore-scripts "$@"
}

make_package() {
    local directory="$1" name="$2" version="$3" exported="$4"
    PACKAGE_DIR="$directory" PACKAGE_NAME="$name" PACKAGE_VERSION="$version" PACKAGE_EXPORT="$exported" node -e '
const fs=require("node:fs"); const path=require("node:path");
fs.rmSync(process.env.PACKAGE_DIR,{recursive:true,force:true}); fs.mkdirSync(process.env.PACKAGE_DIR,{recursive:true});
fs.writeFileSync(path.join(process.env.PACKAGE_DIR,"package.json"),JSON.stringify({name:process.env.PACKAGE_NAME,version:process.env.PACKAGE_VERSION,main:"index.js"}));
fs.writeFileSync(path.join(process.env.PACKAGE_DIR,"index.js"),`module.exports = ${JSON.stringify(process.env.PACKAGE_EXPORT)};\n`);
'
}

publish_package() {
    local repository="$1" directory="$2" config
    config="$WORK_DIR/$repository.npmrc"
    npm_config "$repository" "$config"
    npm_isolated "$config" publish "$directory" --registry "$(registry_url "$repository")" --access public
}

fresh_install() {
    local repository="$1" package_spec="$2" output="$3" install_dir config
    install_dir="$WORK_DIR/install/$output"
    config="$WORK_DIR/$repository.npmrc"
    npm_config "$repository" "$config"
    INSTALL_DIR="$install_dir" node -e 'const fs=require("node:fs"); fs.rmSync(process.env.INSTALL_DIR,{recursive:true,force:true}); fs.mkdirSync(process.env.INSTALL_DIR,{recursive:true}); fs.writeFileSync(process.env.INSTALL_DIR+"/package.json","{\"private\":true}");'
    HOME="$HOME_DIR" npm_config_cache="$WORK_DIR/cache-$output" npm_config_userconfig="$config" \
        npm --silent --ignore-scripts --prefix "$install_dir" install "$package_spec" \
        --registry "$(registry_url "$repository")" --no-package-lock
    PACKAGE_NAME="${package_spec%@*}"
    if [[ "$package_spec" == @* ]]; then PACKAGE_NAME="${package_spec%@*}"; fi
    INSTALL_DIR="$install_dir" PACKAGE_NAME="$PACKAGE_NAME" node -e '
const value=require(require("node:path").resolve(process.env.INSTALL_DIR,"node_modules",process.env.PACKAGE_NAME)); process.stdout.write(String(value));
' > "$WORK_DIR/$output.txt"
}

upstream_control() {
    curl --fail --silent --show-error --request POST --output /dev/null \
        "$ACCEPTANCE_UPSTREAM_URL/__acceptance/$1"
}

create_repo '{"name":"accept-npm-hosted","format":"npm","repo_type":"hosted","visibility":"public","overwrite_policy":"deny"}'
create_repo "{\"name\":\"accept-npm-proxy\",\"format\":\"npm\",\"repo_type\":\"proxy\",\"visibility\":\"public\",\"upstream_url\":\"$ACCEPTANCE_UPSTREAM_URL/npm\"}"
create_repo '{"name":"accept-npm-first","format":"npm","repo_type":"hosted","visibility":"public","overwrite_policy":"deny"}'
create_repo '{"name":"accept-npm-second","format":"npm","repo_type":"hosted","visibility":"public","overwrite_policy":"deny"}'
create_repo '{"name":"accept-npm-group","format":"npm","repo_type":"group","visibility":"public","members":["accept-npm-first","accept-npm-second"]}'

make_package "$WORK_DIR/packages/hosted" hosted-demo 1.0.0 hosted-value
publish_package accept-npm-hosted "$WORK_DIR/packages/hosted"
fresh_install accept-npm-hosted hosted-demo@1.0.0 hosted
[ "$(<"$WORK_DIR/hosted.txt")" = hosted-value ] || fail 'hosted publish/install 内容不一致'

make_package "$WORK_DIR/packages/scoped" @acceptance/scoped-demo 1.0.0 scoped-value
publish_package accept-npm-hosted "$WORK_DIR/packages/scoped"
fresh_install accept-npm-hosted @acceptance/scoped-demo@1.0.0 scoped
[ "$(<"$WORK_DIR/scoped.txt")" = scoped-value ] || fail 'scoped publish/install 内容不一致'

upstream_control reset
fresh_install accept-npm-proxy upstream-only@1.0.0 proxy-first
[ "$(<"$WORK_DIR/proxy-first.txt")" = upstream-only ] || fail 'proxy 首次 install 内容不一致'
upstream_control disable
fresh_install accept-npm-proxy upstream-only@1.0.0 proxy-offline
[ "$(<"$WORK_DIR/proxy-offline.txt")" = upstream-only ] || fail 'proxy 断源缓存 install 失败'
upstream_control enable

make_package "$WORK_DIR/packages/conflict-first" group-conflict 1.0.0 first-member
make_package "$WORK_DIR/packages/conflict-second" group-conflict 1.0.0 second-member
make_package "$WORK_DIR/packages/group-only" group-only 1.0.0 second-only
publish_package accept-npm-first "$WORK_DIR/packages/conflict-first"
publish_package accept-npm-second "$WORK_DIR/packages/conflict-second"
publish_package accept-npm-second "$WORK_DIR/packages/group-only"
fresh_install accept-npm-group group-conflict@1.0.0 group-conflict
[ "$(<"$WORK_DIR/group-conflict.txt")" = first-member ] || fail 'group 版本冲突未按前序成员提供'
fresh_install accept-npm-group group-only@1.0.0 group-only
[ "$(<"$WORK_DIR/group-only.txt")" = second-only ] || fail 'group 未安装后序独有版本'
curl --fail --silent --show-error "$ACCEPTANCE_BASE_URL/repository/accept-npm-group/group-only" --output "$WORK_DIR/group-packument.json"
PACKUMENT="$WORK_DIR/group-packument.json" BASE="$ACCEPTANCE_BASE_URL/repository/accept-npm-group/" node -e '
const fs=require("node:fs"); const value=JSON.parse(fs.readFileSync(process.env.PACKUMENT,"utf8"));
const url=value.versions["1.0.0"].dist.tarball;
if (!url.startsWith(process.env.BASE) || url.includes("accept-npm-second")) process.exit(1);
' || fail 'group packument tarball 地址未重写为 group 自身'

printf 'npm 官方客户端验收通过\n'
