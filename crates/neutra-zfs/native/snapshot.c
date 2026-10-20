#include "reader.h"
#include <sys/zfs_ioctl.h>

static int check_encryption(libzfs_handle_t *handle, const char *dataset)
{
    zfs_handle_t *filesystem = zfs_open(handle, dataset, ZFS_TYPE_FILESYSTEM | ZFS_TYPE_SNAPSHOT);
    if (filesystem == NULL)
        return (ENOENT);
    char value[ZFS_MAXPROPLEN];
    int error = zfs_prop_get(filesystem, ZFS_PROP_ENCRYPTION, value, sizeof (value),
        NULL, NULL, 0, B_TRUE);
    zfs_close(filesystem);
    if (error != 0)
        return (EIO);
    if (strcmp(value, "off") != 0) {
        /* ponytail: encrypted metadata needs secure keys or a kernel reader; kernel keys are not reusable here. */
        fprintf(stderr, "Encrypted ZFS datasets are not supported by this metadata reader; "
            "kernel-loaded keys cannot be reused by read-only libzpool\n");
        return (ENOTSUP);
    }
    return (0);
}

int ns_snapshot(libzfs_handle_t *handle, const char *dataset, char *name,
    size_t capacity, int *cleanup)
{
    int error = check_encryption(handle, dataset);
    if (error != 0)
        return (error);
    if (strchr(dataset, '@') != NULL) {
        if (strlcpy(name, dataset, capacity) >= capacity)
            return (ENAMETOOLONG);
        return (0);
    }
    zfs_cmd_t command = {0};
    if (strlcpy(command.zc_name, dataset, sizeof (command.zc_name)) >= sizeof (command.zc_name))
        return (ENAMETOOLONG);
    *cleanup = open("/dev/zfs", O_RDWR | O_CLOEXEC);
    if (*cleanup < 0)
        return (errno);
    command.zc_cleanup_fd = *cleanup;
    snprintf(command.zc_value, sizeof (command.zc_value), "neutra-%ld", (long)getpid());
    if (zfs_ioctl(handle, ZFS_IOC_TMP_SNAPSHOT, &command) != 0)
        return (errno);
    int length = snprintf(name, capacity, "%s@%s", dataset, command.zc_value);
    return (length < 0 || (size_t)length >= capacity ? ENAMETOOLONG : 0);
}
