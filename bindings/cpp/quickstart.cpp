// OryqDB C++17 RAII Quickstart Example (C ABI v1.1)
//
// Build (Linux/macOS):
//   cargo build -p oryq-ffi
//   c++ -std=c++17 -Wall -Wextra -Werror -Iinclude bindings/cpp/quickstart.cpp -Ltarget/debug -loryq -Wl,-rpath,target/debug -lm -o quickstart_cpp
//   ./quickstart_cpp /tmp/example_cpp.oryq

#include "oryq.h"

#include <iostream>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>

namespace oryq {

inline void check(oryq_status st, const char* op) {
    if (st != ORYQ_OK) {
        const char* msg = oryq_last_error_message();
        throw std::runtime_error(std::string(op) + ": " + (msg ? msg : "(no error message)"));
    }
}

struct DbDeleter {
    void operator()(OryqDb* p) const noexcept {
        if (p) oryq_db_close(p);
    }
};
struct ReadTxnDeleter {
    void operator()(OryqReadTxn* p) const noexcept {
        if (p) oryq_read_txn_close(p);
    }
};
struct WriteTxnRollback {
    void operator()(OryqWriteTxn* p) const noexcept {
        if (p) oryq_write_txn_rollback(p);
    }
};
struct CursorDeleter {
    void operator()(OryqCursor* p) const noexcept {
        if (p) oryq_cursor_close(p);
    }
};

using UniqueDb = std::unique_ptr<OryqDb, DbDeleter>;
using UniqueReadTxn = std::unique_ptr<OryqReadTxn, ReadTxnDeleter>;
using UniqueWriteTxn = std::unique_ptr<OryqWriteTxn, WriteTxnRollback>;
using UniqueCursor = std::unique_ptr<OryqCursor, CursorDeleter>;

}  // namespace oryq

int main(int argc, char** argv) {
    const char* db_path = (argc > 1) ? argv[1] : "quickstart_cpp.oryq";

    OryqDb* raw_db = nullptr;
    oryq::check(oryq_db_open(db_path, &raw_db), "oryq_db_open");
    oryq::UniqueDb db(raw_db);

    {
        OryqWriteTxn* raw_wtx = nullptr;
        oryq::check(oryq_write_txn_begin(db.get(), &raw_wtx), "oryq_write_txn_begin");
        oryq::UniqueWriteTxn wtx(raw_wtx);

        oryq::check(oryq_create_collection(wtx.get(), "notes"), "create_collection");
        oryq::check(oryq_insert_json(wtx.get(), "notes",
                    R"({"_id":1,"title":"Hello C++17","priority":10})"),
                    "insert_json");

        // Commit transfers ownership of the handle out of the rollback guard.
        oryq::check(oryq_write_txn_commit(wtx.release()), "write_txn_commit");
    }

    {
        OryqReadTxn* raw_rtx = nullptr;
        oryq::check(oryq_read_txn_begin(db.get(), &raw_rtx), "oryq_read_txn_begin");
        oryq::UniqueReadTxn rtx(raw_rtx);

        OryqCursor* raw_cur = nullptr;
        oryq::check(oryq_find_json(rtx.get(), "notes", R"({"_id":1})", &raw_cur), "find_json");
        oryq::UniqueCursor cur(raw_cur);

        oryq::check(oryq_cursor_next(cur.get()), "cursor_next");
        const char* ptr = nullptr;
        size_t len = 0;
        oryq::check(oryq_cursor_get_utf8(cur.get(), "title", &ptr, &len), "get_utf8");
        int64_t prio = 0;
        oryq::check(oryq_cursor_get_i64(cur.get(), "priority", &prio), "get_i64");

        std::string_view title(ptr, len);
        std::cout << "C++17 read: title=\"" << title << "\" priority=" << prio << "\n";
        if (title != "Hello C++17" || prio != 10) {
            return 1;
        }
    }

    bool ok = false;
    oryq::check(oryq_db_integrity_check(db.get(), &ok), "integrity_check");
    return ok ? 0 : 1;
}
