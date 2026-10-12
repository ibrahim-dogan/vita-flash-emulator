// RuffleVita's home screen bubble installer.
//
// RuffleVita unpacks a bubble into ux0:data/rufflevita/bubble_pkg (with
// sce_sys/package/head.bin), writes its title ID to bubble_install.txt and
// replaces itself with this program (sceAppMgrLoadExec). We install the
// bubble with the system's promoter, as VitaShell does, write the outcome to
// bubble_result.txt and go back to RuffleVita.
//
// This has to be its own process: RuffleVita keeps nearly all the memory for
// games, and when the promoter couldn't get any from inside it
// (SCE_KERNEL_ERROR_NO_FREE_PHYSICAL_PAGE) the whole Vita reset.
//
// While it works it shows bubble_screen.raw, a 960x544 RGBA picture
// RuffleVita drew ("Adding ... to the home screen"), and logs each step's
// time to bubble_installer.log.
//
// Signed as unsafe homebrew: the promoter needs it. With HENkaku's "Enable
// Unsafe Homebrew" off this runs with safe permissions, loading the promoter
// fails, and RuffleVita saves a VPK instead.

#include <psp2/appmgr.h>
#include <psp2/display.h>
#include <psp2/kernel/sysmem.h>
#include <psp2/io/fcntl.h>
#include <psp2/kernel/processmgr.h>
#include <psp2/promoterutil.h>
#include <psp2/sysmodule.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#define DATA "ux0:data/rufflevita/"

static SceUID log_fd = -1;

static void say(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *fmt, ...) {
	if (log_fd < 0)
		return;
	char line[160];
	int n = snprintf(line, sizeof(line), "%8llu ms  ", (unsigned long long)(sceKernelGetProcessTimeWide() / 1000));
	va_list ap;
	va_start(ap, fmt);
	n += vsnprintf(line + n, sizeof(line) - n - 1, fmt, ap);
	va_end(ap);
	if (n > (int)sizeof(line) - 2)
		n = sizeof(line) - 2;
	line[n++] = '\n';
	sceIoWrite(log_fd, line, n);
}

// Puts RuffleVita's picture on screen, so it isn't black while we work.
static void show_screen(void) {
	SceUID fd = sceIoOpen(DATA "bubble_screen.raw", SCE_O_RDONLY, 0);
	if (fd < 0)
		return;
	SceUID block = sceKernelAllocMemBlock("screen", SCE_KERNEL_MEMBLOCK_TYPE_USER_CDRAM_RW, 0x200000, NULL);
	void *base = NULL;
	if (block >= 0 && sceKernelGetMemBlockBase(block, &base) >= 0) {
		sceIoRead(fd, base, 960 * 544 * 4);
		SceDisplayFrameBuf fb = {sizeof(fb), base, 960, SCE_DISPLAY_PIXELFORMAT_A8B8G8R8, 960, 544};
		int r = sceDisplaySetFrameBuf(&fb, SCE_DISPLAY_SETBUF_NEXTFRAME);
		say("screen 0x%08X", r);
	} else {
		say("screen memory 0x%08X", block);
	}
	sceIoClose(fd);
}

static void write_result(const char *text) {
	SceUID fd = sceIoOpen(DATA "bubble_result.txt", SCE_O_WRONLY | SCE_O_CREAT | SCE_O_TRUNC, 0666);
	if (fd >= 0) {
		sceIoWrite(fd, text, strlen(text));
		sceIoClose(fd);
	}
}

static int load_paf(void) {
	// VitaShell's arguments.
	static uint32_t argp[] = {0x180000, -1, -1, 1, -1, -1};
	int result = -1;
	uint32_t buf[4] = {sizeof(buf), (uint32_t)&result, -1, -1};
	return sceSysmoduleLoadModuleInternalWithArg(SCE_SYSMODULE_INTERNAL_PAF, sizeof(argp), argp, (void *)buf);
}

static void unload_paf(void) {
	uint32_t buf = 0;
	sceSysmoduleUnloadModuleInternalWithArg(SCE_SYSMODULE_INTERNAL_PAF, 0, NULL, (void *)&buf);
}

// Returns the result line for RuffleVita.
static void install(const char *title_id, char *out, int out_len) {
	int r = load_paf();
	say("paf 0x%08X", r);
	if (r < 0) {
		snprintf(out, out_len, "denied paf 0x%08X", r);
		return;
	}
	r = sceSysmoduleLoadModuleInternal(SCE_SYSMODULE_INTERNAL_PROMOTER_UTIL);
	say("promoter 0x%08X", r);
	if (r < 0) {
		snprintf(out, out_len, "denied promoter 0x%08X", r);
		unload_paf();
		return;
	}
	r = scePromoterUtilityInit();
	say("init 0x%08X", r);
	if (r < 0) {
		snprintf(out, out_len, "error init 0x%08X", r);
	} else {
		// A bubble made again for the same game replaces the old one.
		int unused = 0;
		if (scePromoterUtilityCheckExist(title_id, &unused) >= 0) {
			r = scePromoterUtilityDeletePkg(title_id);
			say("delete old 0x%08X", r);
		}
		r = scePromoterUtilityPromotePkgWithRif(DATA "bubble_pkg", 1);
		say("promote 0x%08X", r);
		if (r < 0)
			snprintf(out, out_len, "error promote 0x%08X", r);
		else
			snprintf(out, out_len, "ok");
		// Report now: RuffleVita should hear about it even if the
		// clean-up below gets stuck.
		write_result(out);
		r = scePromoterUtilityExit();
		say("exit 0x%08X", r);
	}
	r = sceSysmoduleUnloadModuleInternal(SCE_SYSMODULE_INTERNAL_PROMOTER_UTIL);
	say("unload promoter 0x%08X", r);
	unload_paf();
	say("unload paf");
}

int main(void) {
	log_fd = sceIoOpen(DATA "bubble_installer.log", SCE_O_WRONLY | SCE_O_CREAT | SCE_O_TRUNC, 0666);
	say("start");
	show_screen();
	char title_id[16] = {0};
	SceUID fd = sceIoOpen(DATA "bubble_install.txt", SCE_O_RDONLY, 0);
	if (fd >= 0) {
		int n = sceIoRead(fd, title_id, 9);
		sceIoClose(fd);
		title_id[n > 0 ? n : 0] = 0;
	}

	char result[64];
	if (strlen(title_id) == 9)
		install(title_id, result, sizeof(result));
	else
		snprintf(result, sizeof(result), "error no title id");
	write_result(result);
	say("result: %s", result);

	int r = sceAppMgrLoadExec("app0:eboot.bin", NULL, NULL);
	say("back to RuffleVita failed 0x%08X", r);
	if (log_fd >= 0)
		sceIoClose(log_fd);
	sceKernelExitProcess(0);
	return 0;
}
