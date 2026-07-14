# Vitis HLS build script
# Запуск: vitis_hls -f run_hls.tcl

open_project evrtck_hls_proj
set_top evrtck_encode_core
add_files evrtck_hls.cpp
add_files evrtck_hls.hpp

open_solution "alveo_u50"
set_part {xcu50-fsvh2104-2-e}
create_clock -period 3.33 -name default   ;# 300 MHz

# C-симуляция (быстро, без синтеза)
csim_design -clean

# Синтез в RTL
csynth_design

# Проверка временных ограничений
# report_timing -file timing_report.txt

close_project
