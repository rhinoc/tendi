#include <libproc.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/file.h>
#include <sys/proc_info.h>
#include <sys/proc.h>
#include <sys/event.h>
#include <unistd.h>

uint64_t tendi_process_start(int pid) {
    struct proc_bsdinfo info;
    if (proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info)) != sizeof(info)) return 0;
    if (info.pbi_status == SZOMB || (info.pbi_flags & PROC_FLAG_INEXIT)) return 0;
    return info.pbi_start_tvsec;
}

int tendi_process_ids(int *ids, int bytes) { return proc_listallpids(ids, bytes); }
int tendi_process_path(int pid, char *path, int bytes) { return proc_pidpath(pid, path, bytes); }

void tendi_process_writable_files(int pid, void (*visit)(const char *, void *), void *context) {
    int bytes = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, NULL, 0);
    if (bytes <= 0) return;
    bytes += 32 * sizeof(struct proc_fdinfo);
    struct proc_fdinfo *fds = malloc(bytes);
    if (!fds) return;
    int count = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, fds, bytes) / sizeof(*fds);
    for (int i = 0; i < count; i++) {
        if (fds[i].proc_fdtype != PROX_FDTYPE_VNODE) continue;
        struct vnode_fdinfowithpath info;
        if (proc_pidfdinfo(pid, fds[i].proc_fd, PROC_PIDFDVNODEPATHINFO, &info, sizeof(info)) != sizeof(info)) continue;
        if (!(info.pfi.fi_openflags & FWRITE)) continue;
        info.pvip.vip_path[sizeof(info.pvip.vip_path) - 1] = 0;
        visit(info.pvip.vip_path, context);
    }
    free(fds);
}

int tendi_process_monitor_create(void) { return kqueue(); }
void tendi_process_monitor_watch(int queue, int pid, int enabled) {
    struct kevent event;
    EV_SET(&event, pid, EVFILT_PROC, enabled ? EV_ADD | EV_ONESHOT : EV_DELETE, NOTE_EXIT, 0, NULL);
    kevent(queue, &event, 1, NULL, 0, NULL);
}
int tendi_process_monitor_changed(int queue) {
    struct kevent events[32];
    struct timespec timeout = {0, 0};
    return kevent(queue, NULL, 0, events, 32, &timeout) > 0;
}
void tendi_process_monitor_close(int queue) { close(queue); }
