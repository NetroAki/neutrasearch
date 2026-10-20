#include "reader.h"

typedef struct ns_frame {
    zap_cursor_t cursor;
    uint64_t object;
    size_t path_length;
    struct ns_frame *previous;
} ns_frame_t;

static ns_frame_t *push(objset_t *os, uint64_t object, size_t length, ns_frame_t *previous)
{
    ns_frame_t *frame = calloc(1, sizeof (*frame));
    if (frame == NULL)
        return (NULL);
    frame->object = object;
    frame->path_length = length;
    frame->previous = previous;
    zap_cursor_init(&frame->cursor, os, object);
    return (frame);
}

static ns_frame_t *pop(ns_frame_t *frame)
{
    ns_frame_t *previous = frame->previous;
    zap_cursor_fini(&frame->cursor);
    free(frame);
    return (previous);
}

static int child_path(char *path, size_t parent_length, const char *name)
{
    size_t length = strlen(name);
    if (length == 0 || strchr(name, '/') != NULL)
        return (EINVAL);
    if (parent_length + length + 1 >= 4096)
        return (ENAMETOOLONG);
    path[parent_length] = '/';
    memcpy(path + parent_length + 1, name, length + 1);
    return (0);
}

int ns_walk(objset_t *os, sa_attr_type_t *attrs, uint64_t root)
{
    boolean_t directory = B_FALSE;
    char path[4096] = {0};
    ns_frame_t *frame = NULL;
    if (fwrite("NSZF0001", 1, 8, stdout) != 8)
        return (EIO);
    int error = ns_record(os, attrs, root, root, "/", &directory);
    if (error != 0 || !directory)
        return (error != 0 ? error : EINVAL);
    frame = push(os, root, 0, NULL);
    if (frame == NULL)
        return (ENOMEM);
    while (frame != NULL) {
        zap_attribute_t attr;
        error = zap_cursor_retrieve(&frame->cursor, &attr);
        if (error == ENOENT) {
            frame = pop(frame);
            continue;
        }
        if (error != 0)
            break;
        zap_cursor_advance(&frame->cursor);
        if (attr.za_integer_length != 8 || attr.za_num_integers != 1) {
            error = EINVAL;
            break;
        }
        error = child_path(path, frame->path_length, attr.za_name);
        if (error != 0)
            break;
        uint64_t object = ZFS_DIRENT_OBJ(attr.za_first_integer);
        error = ns_record(os, attrs, object, frame->object, path, &directory);
        if (error != 0)
            break;
        if (directory) {
            ns_frame_t *next = push(os, object, strlen(path), frame);
            if (next == NULL) {
                error = ENOMEM;
                break;
            }
            frame = next;
        }
    }
    while (frame != NULL)
        frame = pop(frame);
    return (error == ENOENT ? ns_finish() : error);
}
