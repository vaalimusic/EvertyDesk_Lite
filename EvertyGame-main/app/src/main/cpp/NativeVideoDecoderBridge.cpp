#include <jni.h>

#include <android/native_window_jni.h>
#include <media/NdkMediaCodec.h>
#include <media/NdkMediaFormat.h>

#include <algorithm>
#include <cctype>
#include <cstring>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace {

struct NativeVideoDecoderContext {
    AMediaCodec* codec = nullptr;
    ANativeWindow* window = nullptr;
    std::mutex mutex;
    std::string decoder_path;
    int output_width = 0;
    int output_height = 0;
};

NativeVideoDecoderContext* GetContext(jlong handle) {
    return reinterpret_cast<NativeVideoDecoderContext*>(handle);
}

void ThrowRuntimeException(JNIEnv* env, const std::string& message) {
    jclass exception_class = env->FindClass("java/lang/RuntimeException");
    if (exception_class == nullptr) {
        env->FatalError(message.c_str());
    }
    env->ThrowNew(exception_class, message.c_str());
}

std::string JStringToStdString(JNIEnv* env, jstring value) {
    if (value == nullptr) {
        return {};
    }

    const char* chars = env->GetStringUTFChars(value, nullptr);
    if (chars == nullptr) {
        return {};
    }

    std::string result(chars);
    env->ReleaseStringUTFChars(value, chars);
    return result;
}

std::string DescribeDecoderPath(const std::string& codec_name) {
    std::string normalized = codec_name;
    for (char& ch : normalized) {
        ch = static_cast<char>(tolower(static_cast<unsigned char>(ch)));
    }

    std::vector<std::string> traits;
    if (normalized.rfind("omx.google.", 0) == 0 || normalized.rfind("c2.android.", 0) == 0) {
        traits.emplace_back("sw");
    } else if (normalized.rfind("omx.", 0) == 0 || normalized.rfind("c2.", 0) == 0) {
        traits.emplace_back("hw");
    }

    if (normalized.find(".qti.") != std::string::npos ||
        normalized.find(".qcom.") != std::string::npos ||
        normalized.find(".mtk.") != std::string::npos ||
        normalized.find(".allwinner.") != std::string::npos ||
        normalized.find(".amlogic.") != std::string::npos) {
        traits.emplace_back("vendor");
    }

    if (traits.empty()) {
        return codec_name;
    }

    std::string joined;
    for (size_t index = 0; index < traits.size(); ++index) {
        if (index > 0) {
            joined += "/";
        }
        joined += traits[index];
    }
    return codec_name + " [" + joined + "]";
}

void UpdateOutputFormatLocked(NativeVideoDecoderContext* context) {
    AMediaFormat* format = AMediaCodec_getOutputFormat(context->codec);
    if (format == nullptr) {
        return;
    }

    int32_t width = 0;
    int32_t height = 0;
    if (AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_WIDTH, &width)) {
        context->output_width = width;
    }
    if (AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_HEIGHT, &height)) {
        context->output_height = height;
    }
    AMediaFormat_delete(format);
}

void DrainOutputsLocked(
    NativeVideoDecoderContext* context,
    int64_t* last_rendered_pts_us,
    int32_t* rendered_frames) {
    AMediaCodecBufferInfo buffer_info{};

    while (true) {
        const ssize_t output_index = AMediaCodec_dequeueOutputBuffer(context->codec, &buffer_info, 0);
        if (output_index >= 0) {
            *last_rendered_pts_us = buffer_info.presentationTimeUs;
            *rendered_frames += 1;
            AMediaCodec_releaseOutputBuffer(context->codec, static_cast<size_t>(output_index), true);
            continue;
        }

        if (output_index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED) {
            UpdateOutputFormatLocked(context);
            continue;
        }

        break;
    }
}

}  // namespace

extern "C" JNIEXPORT jlong JNICALL
Java_com_everty_evertygame_receiver_NativeVideoDecoderBridge_nativeCreateDecoder(
    JNIEnv* env,
    jobject /* thiz */,
    jstring codec_mime,
    jint width,
    jint height,
    jobject surface,
    jobjectArray codec_specific_data,
    jstring codec_name) {
    const std::string mime = JStringToStdString(env, codec_mime);
    const std::string requested_codec_name = JStringToStdString(env, codec_name);
    if (mime.empty()) {
        ThrowRuntimeException(env, "Native decoder mime is empty");
        return 0;
    }
    if (surface == nullptr) {
        ThrowRuntimeException(env, "Native decoder surface is null");
        return 0;
    }

    ANativeWindow* native_window = ANativeWindow_fromSurface(env, surface);
    if (native_window == nullptr) {
        ThrowRuntimeException(env, "Failed to acquire ANativeWindow from Surface");
        return 0;
    }

    AMediaCodec* codec = requested_codec_name.empty()
        ? AMediaCodec_createDecoderByType(mime.c_str())
        : AMediaCodec_createCodecByName(requested_codec_name.c_str());
    if (codec == nullptr) {
        ANativeWindow_release(native_window);
        ThrowRuntimeException(env, "Failed to create native decoder for " + mime);
        return 0;
    }

    AMediaFormat* format = AMediaFormat_new();
    AMediaFormat_setString(format, AMEDIAFORMAT_KEY_MIME, mime.c_str());
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_WIDTH, width);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_HEIGHT, height);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_MAX_INPUT_SIZE, std::max(1, width * height * 3 / 2));

    if (codec_specific_data != nullptr) {
        const jsize count = env->GetArrayLength(codec_specific_data);
        for (jsize index = 0; index < count; ++index) {
            auto buffer = static_cast<jbyteArray>(env->GetObjectArrayElement(codec_specific_data, index));
            if (buffer == nullptr) {
                continue;
            }

            const jsize length = env->GetArrayLength(buffer);
            std::vector<uint8_t> bytes(static_cast<size_t>(length));
            env->GetByteArrayRegion(buffer, 0, length, reinterpret_cast<jbyte*>(bytes.data()));
            const std::string key = "csd-" + std::to_string(index);
            AMediaFormat_setBuffer(format, key.c_str(), bytes.data(), bytes.size());
            env->DeleteLocalRef(buffer);
        }
    }

    media_status_t status = AMediaCodec_configure(codec, format, native_window, nullptr, 0);
    if (status == AMEDIA_OK) {
        status = AMediaCodec_start(codec);
    }
    AMediaFormat_delete(format);

    if (status != AMEDIA_OK) {
        AMediaCodec_delete(codec);
        ANativeWindow_release(native_window);
        ThrowRuntimeException(env, "Failed to start native decoder");
        return 0;
    }

    std::unique_ptr<NativeVideoDecoderContext> context(new NativeVideoDecoderContext());
    context->codec = codec;
    context->window = native_window;
    context->decoder_path = DescribeDecoderPath(requested_codec_name.empty() ? mime : requested_codec_name);
    UpdateOutputFormatLocked(context.get());
    return reinterpret_cast<jlong>(context.release());
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_com_everty_evertygame_receiver_NativeVideoDecoderBridge_nativeDecodeAccessUnit(
    JNIEnv* env,
    jobject /* thiz */,
    jlong native_handle,
    jbyteArray access_unit,
    jint access_unit_size,
    jlong presentation_time_us) {
    NativeVideoDecoderContext* context = GetContext(native_handle);
    if (context == nullptr || context->codec == nullptr) {
        ThrowRuntimeException(env, "Native decoder is not initialized");
        return nullptr;
    }

    std::vector<uint8_t> bytes(static_cast<size_t>(access_unit_size));
    env->GetByteArrayRegion(access_unit, 0, access_unit_size, reinterpret_cast<jbyte*>(bytes.data()));

    int64_t last_rendered_pts_us = -1;
    int32_t rendered_frames = 0;
    int32_t status_code = 0;

    {
        std::lock_guard<std::mutex> lock(context->mutex);
        DrainOutputsLocked(context, &last_rendered_pts_us, &rendered_frames);

        const ssize_t input_index = AMediaCodec_dequeueInputBuffer(context->codec, 0);
        if (input_index < 0) {
            status_code = -1;
        } else {
            size_t input_capacity = 0;
            uint8_t* input_buffer = AMediaCodec_getInputBuffer(context->codec, static_cast<size_t>(input_index), &input_capacity);
            if (input_buffer == nullptr || input_capacity < static_cast<size_t>(access_unit_size)) {
                status_code = -2;
            } else {
                memcpy(input_buffer, bytes.data(), static_cast<size_t>(access_unit_size));
                const media_status_t queue_status =
                    AMediaCodec_queueInputBuffer(
                        context->codec,
                        static_cast<size_t>(input_index),
                        0,
                        static_cast<size_t>(access_unit_size),
                        presentation_time_us,
                        0);
                if (queue_status != AMEDIA_OK) {
                    status_code = -3;
                } else {
                    DrainOutputsLocked(context, &last_rendered_pts_us, &rendered_frames);
                }
            }
        }
    }

    jlongArray result = env->NewLongArray(5);
    if (result == nullptr) {
        return nullptr;
    }
    const jlong values[5] = {
        static_cast<jlong>(status_code),
        static_cast<jlong>(rendered_frames),
        static_cast<jlong>(last_rendered_pts_us),
        static_cast<jlong>(context->output_width),
        static_cast<jlong>(context->output_height),
    };
    env->SetLongArrayRegion(result, 0, 5, values);
    return result;
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_everty_evertygame_receiver_NativeVideoDecoderBridge_nativeGetDecoderPath(
    JNIEnv* env,
    jobject /* thiz */,
    jlong native_handle) {
    NativeVideoDecoderContext* context = GetContext(native_handle);
    if (context == nullptr) {
        return env->NewStringUTF("-");
    }
    return env->NewStringUTF(context->decoder_path.c_str());
}

extern "C" JNIEXPORT void JNICALL
Java_com_everty_evertygame_receiver_NativeVideoDecoderBridge_nativeReleaseDecoder(
    JNIEnv* /* env */,
    jobject /* thiz */,
    jlong native_handle) {
    std::unique_ptr<NativeVideoDecoderContext> context(GetContext(native_handle));
    if (!context) {
        return;
    }

    std::lock_guard<std::mutex> lock(context->mutex);
    if (context->codec != nullptr) {
        AMediaCodec_stop(context->codec);
        AMediaCodec_delete(context->codec);
        context->codec = nullptr;
    }
    if (context->window != nullptr) {
        ANativeWindow_release(context->window);
        context->window = nullptr;
    }
}
