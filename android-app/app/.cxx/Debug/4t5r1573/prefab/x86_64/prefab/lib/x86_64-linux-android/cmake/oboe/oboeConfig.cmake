if(NOT TARGET oboe::oboe)
add_library(oboe::oboe SHARED IMPORTED)
set_target_properties(oboe::oboe PROPERTIES
    IMPORTED_LOCATION "C:/Users/Adarsh/.gradle/caches/9.7.1/transforms/e0536609b184d931da1744002371819a/transformed/oboe-1.9.3/prefab/modules/oboe/libs/android.x86_64/liboboe.so"
    INTERFACE_INCLUDE_DIRECTORIES "C:/Users/Adarsh/.gradle/caches/9.7.1/transforms/e0536609b184d931da1744002371819a/transformed/oboe-1.9.3/prefab/modules/oboe/include"
    INTERFACE_LINK_LIBRARIES ""
)
endif()

