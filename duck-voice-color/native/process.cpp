// SPDX-License-Identifier: GPL-3.0-or-later
#include "../vendor/rubberband/rubberband/RubberBandStretcher.h"
#include <algorithm>
#include <cstddef>

// A separate executable owns this library. No Rubber Band code links into robotd or sounds.
extern "C" int duck_voice_color(const float *input, size_t count, float *output,
                                unsigned int rate, double formant) noexcept {
    try {
        using R = RubberBand::RubberBandStretcher;
        // FormantPreserved is required here: without it, R3 bypasses the intended effect
        // at unity pitch/time. The explicit formant ratio changes resonance without changing pitch.
        R processor(rate, 1, R::OptionEngineFiner | R::OptionThreadingNever |
                              R::OptionFormantPreserved, 1.0, 1.0);
        processor.setDebugLevel(-1);
        processor.setFormantScale(formant);
        processor.setExpectedInputDuration(count);
        processor.setMaxProcessSize(1024);
        const float *study[] = {input};
        processor.study(study, count, true);
        size_t received = 0;
        for (size_t offset = 0; offset < count; offset += 1024) {
            size_t block = std::min(size_t(1024), count - offset);
            const float *in[] = {input + offset};
            processor.process(in, block, offset + block == count);
            int available;
            while ((available = processor.available()) > 0) {
                if (size_t(available) > count - received) return 2;
                float *out[] = {output + received};
                size_t n = processor.retrieve(out, size_t(available));
                if (n == 0) return 3;
                received += n;
            }
        }
        return received == count ? 0 : 4;
    } catch (...) {
        return 1;
    }
}
