# Sourced (hidden) by the Perforce tapes after setup.sh: a Perforce server on
# loopback, a small game project submitted to it in two changes, a teammate
# (alex) who has one file opened and a map locked, the built-in perforce
# plugin turned on, and the shell in the workspace. No git here: Perforce
# alone tells the file manager and the editor what changed.
p4_quiet() { "$@" >/dev/null 2>&1; }

export P4PORT=127.0.0.1:1666 P4USER=demo P4CLIENT=demo-game
P4ROOT_DIR=/tmp/p4root
WS=/tmp/demo/game
ALEX_WS=/tmp/p4-alex
rm -rf "$P4ROOT_DIR" "$WS" "$ALEX_WS"
mkdir -p "$P4ROOT_DIR" "$WS" "$ALEX_WS"
p4d -r "$P4ROOT_DIR" -p "$P4PORT" -L "$P4ROOT_DIR/log" -d >/dev/null 2>&1
for _ in $(seq 50); do p4_quiet p4 -u demo info && break; sleep 0.1; done

# client <name> <user> <root>
p4_client() {
    p4 -u "$2" client -o "$1" | sed "s|^Root:.*|Root:\t$3|" | p4 -u "$2" client -i >/dev/null
}
p4_client demo-game demo "$WS"
p4_client alex-game alex "$ALEX_WS"

cd "$WS"
mkdir -p Source/Game Source/UI Config Content/Maps
cat > Source/Game/Health.h <<'EOF'
#pragma once

class Health {
public:
    explicit Health(int max);

    void Damage(int amount);
    void Heal(int amount);
    bool IsDead() const { return current_ <= 0; }
    int Current() const { return current_; }

private:
    int max_;
    int current_;
};
EOF
cat > Source/Game/Health.cpp <<'EOF'
#include "Health.h"

#include <algorithm>

Health::Health(int max) : max_(max), current_(max) {}

void Health::Damage(int amount)
{
    current_ = std::max(0, current_ - amount);
}

void Health::Heal(int amount)
{
    current_ = std::min(max_, current_ + amount);
}
EOF
cat > Source/Game/Player.h <<'EOF'
#pragma once

#include "Health.h"
#include "Weapon.h"

class Player {
public:
    Player();

    void Tick(float dt);
    void Fire();

private:
    Health health_;
    Weapon weapon_;
    float speed_ = 4.5f;
};
EOF
cat > Source/Game/Player.cpp <<'EOF'
#include "Player.h"

Player::Player() : health_(100), weapon_(Weapon::Rifle()) {}

void Player::Tick(float dt)
{
    weapon_.Cool(dt);
}

void Player::Fire()
{
    if (!health_.IsDead()) {
        weapon_.Fire();
    }
}
EOF
cat > Source/Game/Weapon.h <<'EOF'
#pragma once

class Weapon {
public:
    static Weapon Rifle() { return Weapon(12, 0.15f); }

    void Fire();
    void Cool(float dt);

private:
    Weapon(int damage, float cooldown) : damage_(damage), cooldown_(cooldown) {}

    int damage_;
    float cooldown_;
    float heat_ = 0.0f;
};
EOF
cat > Source/Game/Weapon.cpp <<'EOF'
#include "Weapon.h"

void Weapon::Fire()
{
    if (heat_ <= 0.0f) {
        heat_ = cooldown_;
    }
}

void Weapon::Cool(float dt)
{
    heat_ = heat_ > dt ? heat_ - dt : 0.0f;
}
EOF
cat > Source/UI/Hud.cpp <<'EOF'
#include "Hud.h"

void Hud::Draw(const Player& player)
{
    DrawBar("HP", player.HealthFraction());
}
EOF
cat > Source/UI/Hud.h <<'EOF'
#pragma once

class Player;

class Hud {
public:
    void Draw(const Player& player);
};
EOF
cat > Source/Main.cpp <<'EOF'
#include "Game/Player.h"

int main()
{
    Player player;
    for (int frame = 0; frame < 600; ++frame) {
        player.Tick(1.0f / 60.0f);
    }
    return 0;
}
EOF
cat > Config/DefaultGame.ini <<'EOF'
[Game]
StartMap=Arena
MaxPlayers=8
RespawnSeconds=5
EOF
head -c 4096 /dev/zero | tr '\0' '\1' > Content/Maps/Arena.umap
p4_quiet p4 add -t text Source/... Config/...
p4_quiet p4 add -t binary+l Content/...
p4_quiet p4 submit -d "Import the arena prototype"
p4_quiet p4 edit Source/Game/Health.cpp Config/DefaultGame.ini
sed -i 's/RespawnSeconds=5/RespawnSeconds=3/' Config/DefaultGame.ini
sed -i 's/current_ = std::max(0, current_ - amount);/if (amount > 0) {\n        current_ = std::max(0, current_ - amount);\n    }/' Source/Game/Health.cpp
p4_quiet p4 submit -d "Shorter respawn, ignore negative damage"

# alex works on the player and holds the map.
( cd "$ALEX_WS" && export P4USER=alex P4CLIENT=alex-game \
    && p4_quiet p4 sync && p4_quiet p4 edit Source/Game/Player.cpp Content/Maps/Arena.umap \
    && p4_quiet p4 lock Content/Maps/Arena.umap )

# Ours already: a numbered change with the weapon tuning in it.
change=$(printf 'Change: new\nClient: demo-game\nUser: demo\nStatus: new\nDescription:\n\tWeapon tuning\n' \
    | p4 change -i | awk '{print $2}')
p4_quiet p4 edit -c "$change" Source/Game/Weapon.h
sed -i 's/Weapon(12, 0.15f)/Weapon(14, 0.12f)/' Source/Game/Weapon.h

# For the tapes that start with a file already opened: Heal guarded the way
# Damage is.
guard_heal() {
    p4 -q edit Source/Game/Health.cpp
    sed -i 's/^    current_ = std::min(max_, current_ + amount);$/    if (amount > 0) {\n        current_ = std::min(max_, current_ + amount);\n    }/' \
        Source/Game/Health.cpp
}

printf '\n[plugins.perforce]\nmode = "on"\n' >> "$HOME/.config/termide/config.toml"
unset -f p4_quiet p4_client
clear
