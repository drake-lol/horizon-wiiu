// Horizon test bridge: an Aroma plugin that lets a computer on the network drive a Wii U for automated tests.
//
// It listens on TCP port 4410 (one client at a time) for text commands, one per line, and answers each with
// a line starting "OK" or "ERR". `wiiu/tools/wiiuctl.py` is the client. Commands:
//
//   PING                          -> OK pong
//   INFO                          -> OK title=<id hex> version=<plugin version>
//   SHOT TV|DRC [scale]           -> OK IMG <width> <height> <bytes>, then the picture as raw RGB rows
//   PRESS <buttons> [ms]          press and release (buttons: A+B+ZR..., default 150 ms)
//   HOLD <buttons>                hold until RELEASE
//   RELEASE [<buttons>|ALL]
//   STICK L|R <x> <y> [ms]        -1..1 each; no ms: until CLEAR
//   TOUCH <x> <y> [ms]            GamePad screen pixels (854x480), default 150 ms
//   CLEAR                         stop every injected input
//   MENU                          back to the Wii U Menu
//   LAUNCH <title id hex>         start a title
//   REBOOT                        restart the console
//   SHUTDOWN                      power off
//
// Pictures come from the frame the application hands to the screen (GX2CopyColorBufferToScanBuffer), so
// they show exactly what the TV or GamePad gets. Inputs are merged into what the GamePad reports (VPADRead),
// so a test can drive any application without a hand on the GamePad.

#include <wups.h>

#include <coreinit/cache.h>
#include <coreinit/launch.h>
#include <coreinit/memdefaultheap.h>
#include <coreinit/time.h>
#include <coreinit/title.h>
#include <gx2/event.h>
#include <gx2/mem.h>
#include <gx2/surface.h>
#include <gx2/swap.h>
#include <gx2/texture.h>
#include <sysapp/launch.h>
#include <vpad/input.h>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

WUPS_PLUGIN_NAME("Horizon test bridge");
WUPS_PLUGIN_DESCRIPTION("Screenshots, GamePad input and power control over the network (TCP 4410), for automated tests");
WUPS_PLUGIN_VERSION("v0.1");
WUPS_PLUGIN_AUTHOR("Horizon");
WUPS_PLUGIN_LICENSE("MIT");

// (wut sockets come with the plugin metadata in this WUPS version)

static constexpr uint16_t PORT    = 4410;
static constexpr const char *VERSION = "0.1";

// ---- injected input ----

struct Timed {
    uint32_t buttons;
    OSTime until;
};

struct Stick {
    bool on = false;
    float x = 0, y = 0;
    OSTime until = 0; // 0: until CLEAR
};

struct Touch {
    bool on = false;
    uint16_t x = 0, y = 0;
    OSTime until = 0;
};

static std::mutex sInputLock;
static uint32_t sHeld = 0;           // HOLD
static std::vector<Timed> sPresses;  // PRESS
static Stick sSticks[2];             // L, R
static Touch sTouch;
static uint32_t sLastInjected = 0;   // for trigger / release edges

static OSTime msFromNow(uint32_t ms) {
    return OSGetTime() + OSMillisecondsToTicks(ms);
}

static bool active(OSTime until, OSTime now) {
    return until == 0 || now < until;
}

static uint32_t buttonsNow(OSTime now) {
    uint32_t b = sHeld;
    for (auto it = sPresses.begin(); it != sPresses.end();) {
        if (now >= it->until) {
            it = sPresses.erase(it);
        } else {
            b |= it->buttons;
            ++it;
        }
    }
    return b;
}

static bool anyInjection(OSTime now) {
    if (sHeld || !sPresses.empty() || sLastInjected) {
        return true;
    }
    for (auto &s : sSticks) {
        if (s.on && active(s.until, now)) {
            return true;
        }
    }
    return sTouch.on && active(sTouch.until, now);
}

DECL_FUNCTION(int32_t, VPADRead, VPADChan chan, VPADStatus *buffers, uint32_t count, VPADReadError *outError) {
    VPADReadError err = VPAD_READ_SUCCESS;
    int32_t n         = real_VPADRead(chan, buffers, count, &err);
    if (chan != VPAD_CHAN_0 || buffers == nullptr || count == 0) {
        if (outError) {
            *outError = err;
        }
        return n;
    }
    std::lock_guard<std::mutex> lock(sInputLock);
    OSTime now = OSGetTime();
    if (!anyInjection(now)) {
        if (outError) {
            *outError = err;
        }
        return n;
    }
    if (n <= 0 || err != VPAD_READ_SUCCESS) {
        // no GamePad sample this time (or none connected): report one made of the injected input alone
        memset(&buffers[0], 0, sizeof(VPADStatus));
        n   = 1;
        err = VPAD_READ_SUCCESS;
    }
    uint32_t b = buttonsNow(now);
    for (int32_t i = 0; i < n && i < (int32_t) count; i++) {
        VPADStatus &s = buffers[i];
        uint32_t real = s.hold;
        s.hold |= b;
        // edges of the injected buttons, on the newest sample only
        if (i == 0) {
            s.trigger |= b & ~sLastInjected & ~real;
            s.release |= sLastInjected & ~b & ~real;
        }
        for (int k = 0; k < 2; k++) {
            Stick &st = sSticks[k];
            if (st.on && active(st.until, now)) {
                VPADVec2D &v = k == 0 ? s.leftStick : s.rightStick;
                v.x          = st.x;
                v.y          = st.y;
            }
        }
        if (sTouch.on && active(sTouch.until, now)) {
            for (VPADTouchData *t : {&s.tpNormal, &s.tpFiltered1, &s.tpFiltered2}) {
                // raw values the calibration hook below recognises and turns back into these pixels
                t->x        = sTouch.x;
                t->y        = sTouch.y;
                t->touched  = 1;
                t->validity = VPAD_VALID;
            }
        }
    }
    sLastInjected = b;
    for (auto &st : sSticks) {
        if (st.on && st.until != 0 && now >= st.until) {
            st.on = false;
        }
    }
    if (sTouch.on && sTouch.until != 0 && now >= sTouch.until) {
        sTouch.on = false;
    }
    if (outError) {
        *outError = err;
    }
    return n;
}

WUPS_MUST_REPLACE(VPADRead, WUPS_LOADER_LIBRARY_VPAD, VPADRead);

DECL_FUNCTION(void, VPADGetTPCalibratedPoint, VPADChan chan, VPADTouchData *calibrated, const VPADTouchData *uncalibrated) {
    {
        std::lock_guard<std::mutex> lock(sInputLock);
        if (sTouch.on && uncalibrated && uncalibrated->touched && uncalibrated->x == sTouch.x && uncalibrated->y == sTouch.y) {
            *calibrated = *uncalibrated;
            return;
        }
    }
    real_VPADGetTPCalibratedPoint(chan, calibrated, uncalibrated);
}

WUPS_MUST_REPLACE(VPADGetTPCalibratedPoint, WUPS_LOADER_LIBRARY_VPAD, VPADGetTPCalibratedPoint);

// ---- screenshots ----

static std::mutex sShotLock;
static std::condition_variable sShotDone;
static int sShotTarget = 0; // GX2_SCAN_TARGET_TV / _DRC wanted; 0: none pending
static uint32_t sShotScale = 1;
static bool sShotReady = false;
static std::string sShotError;
static uint32_t sShotW = 0, sShotH = 0;
static std::vector<uint8_t> sShotRgb;

/// Read the frame last sent to the screen. The scan buffers are linear memory the CPU reads, so this asks no
/// work of the GPU (an earlier version copied the colour buffer with GX2CopySurface, which hung the GPU on
/// real consoles).
static void capture(GX2ScanTarget target) {
    GX2Texture tex{};
    if (!GX2GetLastFrame(target, &tex) || !tex.surface.image) {
        sShotError = "no frame presented yet";
        return;
    }
    const GX2Surface &src = tex.surface;
    if (src.tileMode != GX2_TILE_MODE_LINEAR_ALIGNED && src.tileMode != GX2_TILE_MODE_LINEAR_SPECIAL) {
        char msg[64];
        snprintf(msg, sizeof msg, "scan buffer tile mode %u not linear", (unsigned) src.tileMode);
        sShotError = msg;
        return;
    }
    uint32_t bytes = src.pitch * src.height * 4;
    DCInvalidateRange(src.image, bytes);

    uint32_t fmt   = src.format & 0x3F;
    uint32_t scale = sShotScale ? sShotScale : 1;
    uint32_t w = src.width / scale, h = src.height / scale;
    sShotRgb.resize((size_t) w * h * 3);
    const uint8_t *px = (const uint8_t *) src.image;
    for (uint32_t y = 0; y < h; y++) {
        const uint8_t *row = px + (size_t) (y * scale) * src.pitch * 4;
        uint8_t *out       = &sShotRgb[(size_t) y * w * 3];
        for (uint32_t x = 0; x < w; x++) {
            const uint8_t *p = row + (size_t) (x * scale) * 4;
            if (fmt == 0x19) {
                // R10G10B10A2, a little-endian dword
                uint32_t v = p[0] | (p[1] << 8) | (p[2] << 16) | ((uint32_t) p[3] << 24);
                out[0]     = (v >> 2) & 0xFF;
                out[1]     = (v >> 12) & 0xFF;
                out[2]     = (v >> 22) & 0xFF;
            } else {
                // R8G8B8A8 (UNORM or SRGB)
                out[0] = p[0];
                out[1] = p[1];
                out[2] = p[2];
            }
            out += 3;
        }
    }
    sShotW = w;
    sShotH = h;
    if (fmt != 0x1A && fmt != 0x19) {
        char msg[64];
        snprintf(msg, sizeof msg, "format %#x read as RGBA8", (unsigned) src.format);
        sShotError = msg; // a warning: the picture is still sent
    }
}

DECL_FUNCTION(void, GX2CopyColorBufferToScanBuffer, const GX2ColorBuffer *colorBuffer, GX2ScanTarget scanTarget) {
    {
        std::unique_lock<std::mutex> lock(sShotLock);
        if (sShotTarget != 0 && (sShotTarget & scanTarget) && colorBuffer) {
            sShotError.clear();
            capture((GX2ScanTarget) (sShotTarget & scanTarget));
            sShotTarget = 0;
            sShotReady  = true;
            sShotDone.notify_all();
        }
    }
    real_GX2CopyColorBufferToScanBuffer(colorBuffer, scanTarget);
}

WUPS_MUST_REPLACE(GX2CopyColorBufferToScanBuffer, WUPS_LOADER_LIBRARY_GX2, GX2CopyColorBufferToScanBuffer);

// ---- the server ----

static std::thread sServer;
static std::atomic<bool> sStop{false};
static std::atomic<int> sListen{-1};
static std::atomic<int> sClient{-1};

static bool sendAll(int fd, const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *) data;
    while (len > 0) {
        int n = send(fd, p, len > 0x10000 ? 0x10000 : len, 0);
        if (n <= 0) {
            return false;
        }
        p += n;
        len -= n;
    }
    return true;
}

static bool reply(int fd, const std::string &line) {
    std::string l = line + "\n";
    return sendAll(fd, l.data(), l.size());
}

static uint32_t parseButtons(const std::string &spec, bool &ok) {
    static const struct {
        const char *name;
        uint32_t bit;
    } names[] = {
            {"A", VPAD_BUTTON_A}, {"B", VPAD_BUTTON_B}, {"X", VPAD_BUTTON_X}, {"Y", VPAD_BUTTON_Y},
            {"L", VPAD_BUTTON_L}, {"R", VPAD_BUTTON_R}, {"ZL", VPAD_BUTTON_ZL}, {"ZR", VPAD_BUTTON_ZR},
            {"PLUS", VPAD_BUTTON_PLUS}, {"MINUS", VPAD_BUTTON_MINUS}, {"HOME", VPAD_BUTTON_HOME},
            {"UP", VPAD_BUTTON_UP}, {"DOWN", VPAD_BUTTON_DOWN}, {"LEFT", VPAD_BUTTON_LEFT},
            {"RIGHT", VPAD_BUTTON_RIGHT}, {"STICK_L", VPAD_BUTTON_STICK_L}, {"STICK_R", VPAD_BUTTON_STICK_R},
            {"TV", VPAD_BUTTON_TV},
    };
    uint32_t bits = 0;
    ok            = true;
    size_t start  = 0;
    while (start <= spec.size()) {
        size_t end       = spec.find('+', start);
        std::string name = spec.substr(start, end == std::string::npos ? std::string::npos : end - start);
        bool found       = false;
        for (auto &n : names) {
            if (strcasecmp(n.name, name.c_str()) == 0) {
                bits |= n.bit;
                found = true;
            }
        }
        if (!found) {
            ok = false;
        }
        if (end == std::string::npos) {
            break;
        }
        start = end + 1;
    }
    return bits;
}

static std::vector<std::string> words(const std::string &line) {
    std::vector<std::string> out;
    size_t i = 0;
    while (i < line.size()) {
        while (i < line.size() && (line[i] == ' ' || line[i] == '\t' || line[i] == '\r')) {
            i++;
        }
        size_t j = i;
        while (j < line.size() && line[j] != ' ' && line[j] != '\t' && line[j] != '\r') {
            j++;
        }
        if (j > i) {
            out.push_back(line.substr(i, j - i));
        }
        i = j;
    }
    return out;
}

/// One command; false ends the connection.
static bool handle(int fd, const std::vector<std::string> &w) {
    if (w.empty()) {
        return true;
    }
    std::string cmd = w[0];
    for (auto &c : cmd) {
        c = toupper(c);
    }
    auto arg = [&](size_t i, const char *def = "") -> std::string { return i < w.size() ? w[i] : def; };

    if (cmd == "PING") {
        return reply(fd, "OK pong");
    }
    if (cmd == "INFO") {
        char msg[96];
        snprintf(msg, sizeof msg, "OK title=%016llx version=%s", (unsigned long long) OSGetTitleID(), VERSION);
        return reply(fd, msg);
    }
    if (cmd == "SHOT") {
        std::string which = arg(1, "TV");
        int target        = (strcasecmp(which.c_str(), "DRC") == 0) ? GX2_SCAN_TARGET_DRC : GX2_SCAN_TARGET_TV;
        uint32_t scale    = (uint32_t) strtoul(arg(2, "1").c_str(), nullptr, 10);
        std::unique_lock<std::mutex> lock(sShotLock);
        sShotReady  = false;
        sShotScale  = scale ? scale : 1;
        sShotTarget = target;
        if (!sShotDone.wait_for(lock, std::chrono::seconds(5), [] { return sShotReady; })) {
            sShotTarget = 0;
            return reply(fd, "ERR no frame presented in 5 s");
        }
        if (sShotRgb.empty()) {
            return reply(fd, "ERR " + sShotError);
        }
        char head[96];
        snprintf(head, sizeof head, "OK IMG %u %u %u%s%s", (unsigned) sShotW, (unsigned) sShotH, (unsigned) sShotRgb.size(),
                 sShotError.empty() ? "" : " ", sShotError.c_str());
        // the header's last field is the byte count; a warning, if any, follows it
        if (!reply(fd, head)) {
            return false;
        }
        bool ok = sendAll(fd, sShotRgb.data(), sShotRgb.size());
        sShotRgb.clear();
        sShotRgb.shrink_to_fit();
        return ok;
    }
    if (cmd == "PRESS" || cmd == "HOLD" || cmd == "RELEASE") {
        bool ok       = true;
        std::string b = arg(1, "ALL");
        uint32_t bits = (cmd == "RELEASE" && strcasecmp(b.c_str(), "ALL") == 0) ? 0xFFFFFFFF : parseButtons(b, ok);
        if (!ok) {
            return reply(fd, "ERR unknown button in " + b);
        }
        std::lock_guard<std::mutex> lock(sInputLock);
        if (cmd == "PRESS") {
            uint32_t ms = (uint32_t) strtoul(arg(2, "150").c_str(), nullptr, 10);
            sPresses.push_back({bits, msFromNow(ms)});
        } else if (cmd == "HOLD") {
            sHeld |= bits;
        } else {
            sHeld &= ~bits;
            if (bits == 0xFFFFFFFF) {
                sPresses.clear();
            }
        }
        return reply(fd, "OK");
    }
    if (cmd == "STICK") {
        std::string which = arg(1, "L");
        int k             = (which == "R" || which == "r") ? 1 : 0;
        std::lock_guard<std::mutex> lock(sInputLock);
        Stick &st = sSticks[k];
        st.on     = true;
        st.x      = strtof(arg(2, "0").c_str(), nullptr);
        st.y      = strtof(arg(3, "0").c_str(), nullptr);
        uint32_t ms = (uint32_t) strtoul(arg(4, "0").c_str(), nullptr, 10);
        st.until  = ms ? msFromNow(ms) : 0;
        return reply(fd, "OK");
    }
    if (cmd == "TOUCH") {
        std::lock_guard<std::mutex> lock(sInputLock);
        sTouch.on = true;
        sTouch.x  = (uint16_t) strtoul(arg(1, "0").c_str(), nullptr, 10);
        sTouch.y  = (uint16_t) strtoul(arg(2, "0").c_str(), nullptr, 10);
        sTouch.until = msFromNow((uint32_t) strtoul(arg(3, "150").c_str(), nullptr, 10));
        return reply(fd, "OK");
    }
    if (cmd == "CLEAR") {
        std::lock_guard<std::mutex> lock(sInputLock);
        sHeld = 0;
        sPresses.clear();
        sSticks[0].on = sSticks[1].on = false;
        sTouch.on                     = false;
        return reply(fd, "OK");
    }
    if (cmd == "MENU") {
        reply(fd, "OK");
        SYSLaunchMenu();
        return false;
    }
    if (cmd == "LAUNCH") {
        uint64_t tid = strtoull(arg(1, "0").c_str(), nullptr, 16);
        if (tid == 0) {
            return reply(fd, "ERR LAUNCH <title id hex>");
        }
        reply(fd, "OK");
        SYSLaunchTitle(tid);
        return false;
    }
    if (cmd == "REBOOT") {
        reply(fd, "OK");
        OSForceFullRelaunch();
        SYSLaunchMenu();
        return false;
    }
    if (cmd == "SHUTDOWN") {
        reply(fd, "OK");
        OSShutdown();
        return false;
    }
    if (cmd == "QUIT") {
        reply(fd, "OK bye");
        return false;
    }
    return reply(fd, "ERR unknown command " + w[0]);
}

static void serveClient(int fd) {
    std::string buf;
    char chunk[512];
    reply(fd, std::string("OK horizon-testbridge ") + VERSION);
    while (!sStop) {
        int n = recv(fd, chunk, sizeof chunk, 0);
        if (n <= 0) {
            break;
        }
        buf.append(chunk, n);
        size_t nl;
        while ((nl = buf.find('\n')) != std::string::npos) {
            std::string line = buf.substr(0, nl);
            buf.erase(0, nl + 1);
            if (!handle(fd, words(line))) {
                return;
            }
        }
    }
}

static void serverMain() {
    int s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (s < 0) {
        return;
    }
    int one = 1;
    setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    sockaddr_in addr{};
    addr.sin_family      = AF_INET;
    addr.sin_port        = htons(PORT);
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    if (bind(s, (sockaddr *) &addr, sizeof addr) < 0 || listen(s, 1) < 0) {
        close(s);
        return;
    }
    sListen = s;
    while (!sStop) {
        int c = accept(s, nullptr, nullptr);
        if (c < 0) {
            if (sStop) {
                break;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            continue;
        }
        sClient = c;
        serveClient(c);
        sClient = -1;
        close(c);
    }
    sListen = -1;
    close(s);
}

static void stopServer() {
    sStop = true;
    int c = sClient.exchange(-1);
    if (c >= 0) {
        shutdown(c, SHUT_RDWR);
    }
    int s = sListen.exchange(-1);
    if (s >= 0) {
        shutdown(s, SHUT_RDWR);
        close(s);
    }
    if (sServer.joinable()) {
        sServer.join();
    }
    {
        std::lock_guard<std::mutex> lock(sInputLock);
        sHeld = 0;
        sPresses.clear();
        sSticks[0].on = sSticks[1].on = false;
        sTouch.on                     = false;
        sLastInjected                 = 0;
    }
}

INITIALIZE_PLUGIN() {}

DEINITIALIZE_PLUGIN() {
    stopServer();
}

// Each application gets the server anew: sockets belong to the application the plugin runs in.
ON_APPLICATION_START() {
    sStop   = false;
    sServer = std::thread(serverMain);
}

ON_APPLICATION_REQUESTS_EXIT() {
    stopServer();
}

ON_APPLICATION_ENDS() {
    stopServer();
}
