#include "reader.h"
#include <libzfs.h>
#include <sys/arc.h>
#include <sys/spa.h>

#ifndef NEUTRA_ZFS_ABI_VERSION
#error Build through packaging/tools/build-zfs-reader.py with version-checked headers
#endif

extern uint64_t zfs_arc_max;
extern uint64_t zfs_arc_min;
extern boolean_t spa_load_verify_dryrun;
extern char *spa_config_path;

static int pool_config(const char *name, const char *dataset, nvlist_t **config,
    char *snapshot, size_t capacity, int *cleanup)
{
    libzfs_handle_t *handle = libzfs_init();
    if (handle == NULL)
        return (ENODEV);
    int error = ns_snapshot(handle, dataset, snapshot, capacity, cleanup);
    zpool_handle_t *pool = error == 0 ? zpool_open(handle, name) : NULL;
    if (error == 0)
        error = pool == NULL ? ENOENT : nvlist_dup(zpool_get_config(pool, NULL), config, 0);
    if (pool != NULL)
        zpool_close(pool);
    libzfs_fini(handle);
    return (error);
}

static int scan_snapshot(const char *name)
{
    objset_t *os = NULL;
    sa_attr_type_t *attrs = NULL;
    uint64_t version = 0, sa_object = 0, root = 0;
    const void *tag = "neutrasearch-zfs";
    int error = dmu_objset_own(name, DMU_OST_ZFS, B_TRUE, B_TRUE, tag, &os);
    if (error != 0)
        return (error);
    error = zap_lookup(os, MASTER_NODE_OBJ, ZPL_VERSION_STR, 8, 1, &version);
    if (error == 0 && version >= ZPL_VERSION_SA)
        error = zap_lookup(os, MASTER_NODE_OBJ, ZFS_SA_ATTRS, 8, 1, &sa_object);
    if (error == 0)
        error = sa_setup(os, sa_object, zfs_attr_table, ZPL_END, &attrs);
    if (error == 0)
        error = zap_lookup(os, MASTER_NODE_OBJ, ZFS_ROOT_OBJ, 8, 1, &root);
    if (error == 0)
        error = ns_walk(os, attrs, root);
    if (attrs != NULL)
        sa_tear_down(os);
    dmu_objset_disown(os, B_TRUE, tag);
    return (error);
}

int main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        printf("neutrasearch-zfs wire=1 openzfs=%s\n", NEUTRA_ZFS_ABI_VERSION);
        return (0);
    }
    if (argc != 2) {
        fprintf(stderr, "A ZFS dataset or snapshot is required\n");
        return (2);
    }
    const char *version = zfs_version_userland();
    if (version == NULL || strstr(version, "zfs-" NEUTRA_ZFS_ABI_VERSION "-") != version) {
        fprintf(stderr, "OpenZFS library version differs from compiled metadata headers\n");
        return (2);
    }
    char pool[ZFS_MAX_DATASET_NAME_LEN];
    size_t length = strcspn(argv[1], "/@");
    if (length == 0 || length >= sizeof (pool))
        return (2);
    memcpy(pool, argv[1], length);
    pool[length] = '\0';
    nvlist_t *config = NULL;
    char snapshot[ZFS_MAX_DATASET_NAME_LEN];
    int cleanup = -1;
    int error = pool_config(pool, argv[1], &config, snapshot, sizeof (snapshot), &cleanup);
    if (error != 0)
        goto failed;
    zfs_arc_max = 64ULL << 20;
    zfs_arc_min = 32ULL << 20;
    spa_load_verify_dryrun = B_TRUE;
    spa_config_path = "/dev/null";
    kernel_init(SPA_MODE_READ);
    error = spa_import(pool, config, NULL, ZFS_IMPORT_SKIP_MMP);
    nvlist_free(config);
    if (error == 0)
        error = scan_snapshot(snapshot);
    kernel_fini();
    if (cleanup >= 0)
        close(cleanup);
    if (error == 0)
        return (0);
failed:
    fprintf(stderr, "ZFS native metadata reader: %s (%d)\n", strerror(error), error);
    return (1);
}
