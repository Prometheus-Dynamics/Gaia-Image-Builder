// castidle: the cast receiver's idle screen.
//
// Draws the receiver's name and IP addresses into the fbdev emulation of the
// KMS driver (/dev/fb0). It never opens the DRM device, so it is never DRM
// master: a receiver's kmssink takes the display while it streams, and when
// it closes the device the kernel restores the fbdev mode, showing this
// screen again. While another process has a /dev/dri node open (a cast is
// playing), the framebuffer is painted black, so no idle text shows around a
// letterboxed video on another plane.
//
// usage: castidle NAME [SUBTITLE]

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <linux/fb.h>
#include <net/if.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

// Terminus 16x32 from the kernel tree (lib/fonts/font_ter16x32.c): 256
// glyphs, 32 rows of 2 bytes each, most significant bit leftmost.
static const uint8_t font[] = {
#include "font_ter16x32.inc"
};
#define GLYPH_W 16
#define GLYPH_H 32
_Static_assert(sizeof(font) == 256 * GLYPH_H * 2, "font_ter16x32 size");

#define POLL_MS 500
// A pan re-commits the fbdev mode; repeat it this often while idle, so the
// screen comes back even if a restore was missed (a sink that crashed).
#define REPAN_EVERY 20

struct rgb {
	uint8_t r, g, b;
};

static const struct rgb BG = {0x0f, 0x14, 0x19};
static const struct rgb FG = {0xf2, 0xf4, 0xf7};
static const struct rgb ACCENT = {0x6c, 0xb6, 0xff};
static const struct rgb MUTED = {0x8b, 0x94, 0x9e};
static const struct rgb BLACK = {0, 0, 0};

struct fb {
	int fd;
	struct fb_var_screeninfo var;
	struct fb_fix_screeninfo fix;
	uint8_t *mem;   // the mapped framebuffer
	uint8_t *back;  // a frame drawn off screen, copied in one go
	size_t size;
	unsigned bpp;   // bytes per pixel
};

static uint32_t pack(const struct fb *fb, struct rgb c)
{
	const struct fb_var_screeninfo *v = &fb->var;
	return ((uint32_t)(c.r >> (8 - v->red.length)) << v->red.offset) |
	       ((uint32_t)(c.g >> (8 - v->green.length)) << v->green.offset) |
	       ((uint32_t)(c.b >> (8 - v->blue.length)) << v->blue.offset);
}

static void put(const struct fb *fb, unsigned x, unsigned y, uint32_t px)
{
	if (x >= fb->var.xres || y >= fb->var.yres)
		return;
	uint8_t *p = fb->back + (size_t)y * fb->fix.line_length + (size_t)x * fb->bpp;
	if (fb->bpp == 4)
		*(uint32_t *)p = px;
	else if (fb->bpp == 2)
		*(uint16_t *)p = (uint16_t)px;
}

static void fill(const struct fb *fb, unsigned x0, unsigned y0, unsigned w,
		 unsigned h, struct rgb c)
{
	uint32_t px = pack(fb, c);
	for (unsigned y = y0; y < y0 + h && y < fb->var.yres; y++)
		for (unsigned x = x0; x < x0 + w && x < fb->var.xres; x++)
			put(fb, x, y, px);
}

static unsigned text_width(const char *s, unsigned scale)
{
	return (unsigned)strlen(s) * GLYPH_W * scale;
}

static void text(const struct fb *fb, unsigned x0, unsigned y0, const char *s,
		 unsigned scale, struct rgb c)
{
	uint32_t px = pack(fb, c);
	for (; *s; s++, x0 += GLYPH_W * scale) {
		const uint8_t *g = font + (size_t)(uint8_t)*s * GLYPH_H * 2;
		for (unsigned row = 0; row < GLYPH_H; row++) {
			unsigned bits = (unsigned)g[row * 2] << 8 | g[row * 2 + 1];
			for (unsigned col = 0; col < GLYPH_W; col++) {
				if (!(bits & (0x8000u >> col)))
					continue;
				for (unsigned dy = 0; dy < scale; dy++)
					for (unsigned dx = 0; dx < scale; dx++)
						put(fb, x0 + col * scale + dx,
						    y0 + row * scale + dy, px);
			}
		}
	}
}

// The largest scale (up to max) at which s fits in width.
static unsigned fit(const char *s, unsigned width, unsigned max)
{
	unsigned scale = max;
	while (scale > 1 && text_width(s, scale) > width)
		scale--;
	return scale;
}

static void centered(const struct fb *fb, unsigned y, const char *s,
		     unsigned scale, struct rgb c)
{
	unsigned w = text_width(s, scale);
	unsigned x = w < fb->var.xres ? (fb->var.xres - w) / 2 : 0;
	text(fb, x, y, s, scale, c);
}

static int fb_open(struct fb *fb)
{
	memset(fb, 0, sizeof(*fb));
	fb->fd = open("/dev/fb0", O_RDWR | O_CLOEXEC);
	if (fb->fd < 0)
		return -1;
	if (ioctl(fb->fd, FBIOGET_VSCREENINFO, &fb->var) < 0 ||
	    ioctl(fb->fd, FBIOGET_FSCREENINFO, &fb->fix) < 0)
		goto fail;
	fb->bpp = fb->var.bits_per_pixel / 8;
	if (fb->bpp != 2 && fb->bpp != 4) {
		fprintf(stderr, "castidle: %u bits per pixel is not supported\n",
			fb->var.bits_per_pixel);
		goto fail;
	}
	fb->size = (size_t)fb->fix.line_length * fb->var.yres;
	fb->mem = mmap(NULL, fb->size, PROT_READ | PROT_WRITE, MAP_SHARED, fb->fd, 0);
	if (fb->mem == MAP_FAILED)
		goto fail;
	fb->back = malloc(fb->size);
	if (!fb->back)
		goto fail;
	return 0;
fail:
	close(fb->fd);
	return -1;
}

// Commit the fbdev mode and plane. Fails (EBUSY) while another process is
// DRM master, which is harmless.
static void fb_show(struct fb *fb)
{
	ioctl(fb->fd, FBIOBLANK, FB_BLANK_UNBLANK);
	fb->var.xoffset = 0;
	fb->var.yoffset = 0;
	ioctl(fb->fd, FBIOPAN_DISPLAY, &fb->var);
}

// True while a process other than this one has a DRM node open.
static int display_busy(void)
{
	DIR *proc = opendir("/proc");
	if (!proc)
		return 0;
	pid_t self = getpid();
	int busy = 0;
	struct dirent *p;
	while (!busy && (p = readdir(proc))) {
		char *end;
		long pid = strtol(p->d_name, &end, 10);
		if (*end || pid <= 0 || pid == self)
			continue;
		char path[64];
		snprintf(path, sizeof(path), "/proc/%ld/fd", pid);
		DIR *fds = opendir(path);
		if (!fds)
			continue;
		struct dirent *f;
		while ((f = readdir(fds))) {
			char link[330], target[64];
			snprintf(link, sizeof(link), "%s/%s", path, f->d_name);
			ssize_t n = readlink(link, target, sizeof(target) - 1);
			if (n <= 0)
				continue;
			target[n] = 0;
			if (!strncmp(target, "/dev/dri/", 9)) {
				busy = 1;
				break;
			}
		}
		closedir(fds);
	}
	closedir(proc);
	return busy;
}

// The IPv4 addresses of the interfaces that are up, separated by spaces.
static void addresses(char *out, size_t len)
{
	struct ifaddrs *ifs, *i;
	out[0] = 0;
	if (getifaddrs(&ifs) < 0)
		return;
	for (i = ifs; i; i = i->ifa_next) {
		if (!i->ifa_addr || i->ifa_addr->sa_family != AF_INET)
			continue;
		if ((i->ifa_flags & IFF_LOOPBACK) || !(i->ifa_flags & IFF_UP))
			continue;
		char ip[INET_ADDRSTRLEN];
		inet_ntop(AF_INET, &((struct sockaddr_in *)i->ifa_addr)->sin_addr,
			  ip, sizeof(ip));
		size_t used = strlen(out);
		snprintf(out + used, len - used, "%s%s", used ? "   " : "", ip);
	}
	freeifaddrs(ifs);
}

static void draw_idle(struct fb *fb, const char *name, const char *subtitle,
		      const char *ips)
{
	unsigned w = fb->var.xres, h = fb->var.yres;
	// Sizes for 1080p, scaled down for smaller modes.
	unsigned big = h >= 1000 ? 4 : h >= 700 ? 3 : 2;
	unsigned mid = big > 2 ? 2 : 1;

	fill(fb, 0, 0, w, h, BG);
	unsigned name_scale = fit(name, w - w / 10, big);
	unsigned y = h / 2 - GLYPH_H * name_scale;
	centered(fb, y, name, name_scale, FG);
	y += GLYPH_H * name_scale + GLYPH_H / 2;
	fill(fb, w / 2 - w / 12, y, w / 6, mid * 2, ACCENT);
	y += GLYPH_H;
	centered(fb, y, subtitle, mid > 1 ? mid - 1 : 1, MUTED);
	y += GLYPH_H * 2;
	if (ips[0])
		centered(fb, y, ips, fit(ips, w - w / 10, mid), ACCENT);
	else
		centered(fb, y, "Waiting for network...", mid, MUTED);
}

int main(int argc, char **argv)
{
	const char *name = argc > 1 && argv[1][0] ? argv[1] : "Cast";
	const char *subtitle = argc > 2 ? argv[2] : "Ready for AirPlay and DLNA";
	struct fb fb;

	while (fb_open(&fb) < 0) {
		// vc4 is a module; mdev loads it during boot.
		sleep(1);
	}

	char ips[256], shown_ips[256] = "";
	int shown = -1; // -1 nothing yet, 0 idle screen, 1 black
	unsigned ticks = 0, repan = 0;
	const struct timespec poll = {0, POLL_MS * 1000000L};

	for (;; nanosleep(&poll, NULL), ticks++) {
		int busy = display_busy();
		addresses(ips, sizeof(ips));
		if (busy) {
			if (shown != 1) {
				fill(&fb, 0, 0, fb.var.xres, fb.var.yres, BLACK);
				memcpy(fb.mem, fb.back, fb.size);
				shown = 1;
			}
			continue;
		}
		if (shown != 0 || strcmp(ips, shown_ips)) {
			draw_idle(&fb, name, subtitle, ips);
			memcpy(fb.mem, fb.back, fb.size);
			snprintf(shown_ips, sizeof(shown_ips), "%s", ips);
			// Coming back from a cast: commit the mode a few times, as
			// the sink may release DRM master a moment after closing.
			repan = shown == 1 ? 4 : 1;
			shown = 0;
		}
		if (repan || ticks % REPAN_EVERY == 0) {
			fb_show(&fb);
			if (repan)
				repan--;
		}
	}
}
