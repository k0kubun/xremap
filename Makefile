current_dir := $(shell pwd)
CSRCS := $(wildcard tools/xremap/*.[ch])
MRBSRCS := $(wildcard mrblib/xremap/*.rb)
MRBCSRCS := $(wildcard src/*.[ch])
# Using master to apply https://github.com/mruby/mruby/pull/3192
REVISION=0ff3ae1fbaed62010c54c43235e29cdc85da2f78
DESTDIR := /usr/local/bin
.PHONY: all clean install

all: xremap

clean:
	rm -rf mruby/build/host src/*.inc

install: xremap
	mv xremap $(DESTDIR)/xremap

xremap: mruby/build/host/bin/xremap
	cp mruby/build/host/bin/xremap xremap

mruby:
	git clone https://github.com/mruby/mruby
	git -C mruby reset --hard $(REVISION)

src/x11_constants_keysymdef.inc:
	cat /usr/include/X11/keysymdef.h | ruby -e 'puts STDIN.read.split("\n").select {|l| l.match(/\A(#define XK_|#ifdef|#endif)/) }.map{|l| l.match(/\A#define XK_/) ? %Q[  define_x11_const(#{l.split(" ")[1]});] : l }.join("\n")' > src/x11_constants_keysymdef.inc

src/x11_constants_X.inc:
	cat /usr/include/X11/X.h | ruby -e 'puts STDIN.read.split("\n").select {|l| l.start_with?("#")}[2..-2].map{|l| l.start_with?("#define") ? %Q[  define_x11_const(#{l.split(" ")[1]});] : l}' > src/x11_constants_X.inc

src/x11_constants_XF86keysym.inc:
	cat /usr/include/X11/XF86keysym.h | ruby -e 'puts STDIN.read.split("\n").select {|l| l.match(/\A(#define XF86XK_)/) }.map{|l| l.match(/\A#define XF*86XK_/) ? %Q[  define_x11_const(#{l.split(" ")[1]});] : l }.join("\n")'	> src/x11_constants_XF86keysym.inc

mruby/build/host/bin/xremap: mruby build_config.rb src/x11_constants_keysymdef.inc src/x11_constants_X.inc src/x11_constants_XF86keysym.inc $(CSRCS) $(MRBSRCS) $(MRBCSRCS)
	cd mruby && MRUBY_CONFIG="$(current_dir)/build_config.rb" make
