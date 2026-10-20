#ifndef NEUTRA_ZFS_READER_H
#define NEUTRA_ZFS_READER_H
#include <sys/zfs_context.h>
#include <sys/dmu.h>
#include <sys/sa.h>
#include <sys/zap.h>
#include <sys/zfs_acl.h>
#include <sys/zfs_sa.h>
#include <sys/zfs_znode.h>
#include <libzfs.h>

int ns_walk(objset_t *os, sa_attr_type_t *attrs, uint64_t root);
int ns_record(objset_t *os, sa_attr_type_t *attrs, uint64_t object,
    uint64_t parent, const char *path, boolean_t *directory);
int ns_finish(void);
int ns_snapshot(libzfs_handle_t *handle, const char *dataset, char *name,
    size_t capacity, int *cleanup);
#endif
