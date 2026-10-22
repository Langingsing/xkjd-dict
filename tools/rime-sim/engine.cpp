// JSON-lines bridge to an isolated librime session. All commits come from process_key.
#include <rime_api.h>
#include <dlfcn.h>
#include <nlohmann/json.hpp>
#include <iostream>
#include <stdexcept>
#include <string>
using nlohmann::json;

int main(int argc, char** argv) {
    try {
        if (argc != 2) throw std::runtime_error("usage: engine ISOLATED_USER_DIR");
        void* lua = dlopen("/usr/lib/rime-plugins/librime-lua.so", RTLD_NOW | RTLD_GLOBAL);
        if (!lua) throw std::runtime_error(dlerror());
        auto require_lua = reinterpret_cast<void (*)()>(dlsym(lua, "_Z23rime_require_module_luav"));
        if (!require_lua) throw std::runtime_error("librime-lua module registration unavailable");
        require_lua();
        auto* api = rime_get_api();
        RIME_STRUCT(RimeTraits, traits);
        const char* modules[] = {"default", "lua", nullptr};
        traits.shared_data_dir = "/usr/share/rime-data";
        traits.user_data_dir = argv[1];
        traits.app_name = "rime.xkjd-simulation";
        traits.modules = modules;
        traits.min_log_level = 2;
        api->setup(&traits);
        api->initialize(&traits);
        std::string line;
        while (std::getline(std::cin, line)) {
            RimeSessionId session = 0;
            try {
                const auto request = json::parse(line);
                session = api->create_session();
                const auto schema = request.at("schema").get<std::string>();
                if (!session || !api->select_schema(session, schema.c_str()))
                    throw std::runtime_error("cannot select schema " + schema);
                api->set_option(session, "ascii_mode", false);
                api->set_option(session, "danzi_mode", request.value("single", false));
                api->set_option(session, "emoji_cn", false);
                api->set_option(session, "jffh", false);
                std::string output;
                json trace = json::array();
                for (const auto& value : request.at("keys")) {
                    int key = value.get<int>();
                    std::string input_before = api->get_input(session) ? api->get_input(session) : "";
                    bool handled = api->process_key(session, key, 0);
                    RIME_STRUCT(RimeCommit, commit);
                    std::string committed;
                    if (api->get_commit(session, &commit)) {
                        committed = commit.text ? commit.text : "";
                        output += committed;
                        api->free_commit(&commit);
                    }
                    // Unhandled keys are client pass-through; never pretend they vanished.
                    if (!handled && key >= 32 && key < 127) output += static_cast<char>(key);
                    if (!handled && key == 0xff0d) output += '\n';
                    if (!handled && key == 0xff09) output += '\t';
                    if (request.value("trace", false))
                        trace.push_back({{"key", key}, {"handled", handled}, {"commit", committed}, {"input_before", input_before}});
                }
                RIME_STRUCT(RimeContext, context);
                json menu = json::array();
                std::string preedit;
                int page = 0, size = 0;
                bool last = true;
                if (!api->get_context(session, &context))
                    throw std::runtime_error("cannot read session context after key replay");
                {
                    preedit = context.composition.preedit ? context.composition.preedit : "";
                    page = context.menu.page_no;
                    size = context.menu.page_size;
                    last = context.menu.is_last_page;
                    for (int i = 0; i < context.menu.num_candidates; ++i)
                        menu.push_back(context.menu.candidates[i].text);
                    api->free_context(&context);
                }
                json response = {{"output", output}, {"preedit", preedit}, {"menu", menu},
                    {"input", api->get_input(session) ? api->get_input(session) : ""},
                    {"page", page}, {"page_size", size}, {"last_page", last}, {"trace", trace}};
                std::cout << response.dump() << std::endl;
            } catch (const std::exception& error) {
                std::cout << json({{"error", error.what()}}).dump() << std::endl;
            }
            if (session) api->destroy_session(session);
        }
        api->finalize();
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
