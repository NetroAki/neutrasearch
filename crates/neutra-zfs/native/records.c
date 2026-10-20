#include "reader.h"
#include <endian.h>

static int write_bytes(const void *bytes, size_t length)
{
    return (fwrite(bytes, 1, length, stdout) == length ? 0 : EIO);
}

int ns_record(objset_t *os, sa_attr_type_t *attrs, uint64_t object,
    uint64_t parent, const char *path, boolean_t *directory)
{
    sa_handle_t *handle = NULL;
    uint64_t mode = 0, size = 0, mtime[2] = {0};
    sa_bulk_attr_t bulk[3];
    int count = 0;
    int error = sa_handle_get(os, object, NULL, SA_HDL_PRIVATE, &handle);
    if (error != 0)
        return (error);
    SA_ADD_BULK_ATTR(bulk, count, attrs[ZPL_MODE], NULL, &mode, 8);
    SA_ADD_BULK_ATTR(bulk, count, attrs[ZPL_SIZE], NULL, &size, 8);
    SA_ADD_BULK_ATTR(bulk, count, attrs[ZPL_MTIME], NULL, mtime, 16);
    error = sa_bulk_lookup(handle, bulk, count);
    uint32_t blocksize = 0;
    u_longlong_t blocks = 0;
    if (error == 0)
        sa_object_size(handle, &blocksize, &blocks);
    sa_handle_destroy(handle);
    if (error != 0)
        return (error);
    if (blocks > UINT64_MAX / 512)
        return (EOVERFLOW);
    uint64_t fields[] = {htole64(size), htole64(blocks * 512),
        htole64(mtime[0]), htole64(mode), htole64(object), htole64(parent)};
    size_t length = strlen(path);
    if (length == 0 || length > 4096)
        return (ENAMETOOLONG);
    uint32_t path_length = htole32((uint32_t)length);
    *directory = (mode & S_IFMT) == S_IFDIR;
    if (write_bytes(&path_length, sizeof (path_length)) != 0 ||
        write_bytes(fields, sizeof (fields)) != 0 || write_bytes(path, length) != 0)
        return (EIO);
    return (0);
}

int ns_finish(void)
{
    uint32_t end = 0;
    return (write_bytes(&end, sizeof (end)) == 0 && fflush(stdout) == 0 ? 0 : EIO);
}
