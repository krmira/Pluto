# Builds the Pluto native C frontend (pluto_interface.c + pluto_bridge.c + pluto_platform.c) with MinGW gcc and Raylib.

CC ?= x86_64-w64-mingw32-gcc
RAYLIB_DIR ?= /c/Users/Admin/AppData/Local/pluto-tools/raylib-5.5_win64_mingw-w64
FRONTEND_DIR := .

PLUTO_EXE := pluto.exe
SOURCES := pluto_interface.c pluto_bridge.c pluto_platform.c

.PHONY: all clean

all: $(PLUTO_EXE)

$(PLUTO_EXE): $(SOURCES) pluto_bridge.h pluto_platform.h pluto_interface.c
	$(CC) -std=c11 -O2 -Wall -Wextra -Wno-unused-parameter -I$(RAYLIB_DIR)/include -I$(FRONTEND_DIR) $(SOURCES) -L$(RAYLIB_DIR)/lib -lraylib -lopengl32 -lgdi32 -lwinmm -lcomdlg32 -o $@

clean:
	rm -f $(PLUTO_EXE)
