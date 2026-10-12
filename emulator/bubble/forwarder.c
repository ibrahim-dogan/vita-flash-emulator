// RuffleVita home screen bubble: opens one game in RuffleVita.
//
// RuffleVita writes the game's path (percent-encoded) to app0:game.txt when
// it makes the bubble; every bubble shares this eboot and differs only in
// that file, its param.sfo and its LiveArea images.

#include <psp2/appmgr.h>
#include <psp2/io/fcntl.h>
#include <psp2/kernel/processmgr.h>
#include <psp2/kernel/threadmgr.h>
#include <string.h>

int main(void) {
	char path[1024] = {0};
	SceUID fd = sceIoOpen("app0:game.txt", SCE_O_RDONLY, 0);
	if (fd >= 0) {
		int n = sceIoRead(fd, path, sizeof(path) - 1);
		sceIoClose(fd);
		path[n > 0 ? n : 0] = 0;
	}

	static const char prefix[] = "psgm:play?titleid=RUFFLEVIT&param=";
	char uri[sizeof(prefix) + sizeof(path)];
	memcpy(uri, prefix, sizeof(prefix) - 1);
	strcpy(uri + sizeof(prefix) - 1, path);

	// A RuffleVita left running in the background would just resume and
	// never see the parameter: close it first.
	sceAppMgrDestroyOtherApp();
	sceKernelDelayThread(250 * 1000);
	sceAppMgrLaunchAppByUri(0xFFFFF, uri);
	sceKernelExitProcess(0);
	return 0;
}
