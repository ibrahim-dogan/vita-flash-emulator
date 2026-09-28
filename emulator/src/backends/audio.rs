use ruffle_core::backend::audio::{
    AudioBackend, AudioMixer, DecodeError, RegisterError, SoundHandle, SoundInstanceHandle,
    SoundStreamInfo, SoundTransform, swf,
};
use ruffle_core::impl_audio_mixer_backend;
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};

const SAMPLE_RATE: u32 = 44100;

pub struct SdlAudioBackend {
    device: AudioDevice<MixerCallback>,
    mixer: AudioMixer,
}

pub struct MixerCallback {
    proxy: ruffle_core::backend::audio::AudioMixerProxy,
}

impl AudioCallback for MixerCallback {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        self.proxy.mix(out);
    }
}

impl SdlAudioBackend {
    pub fn new(sdl2_audio: &sdl2::AudioSubsystem) -> Result<Self, String> {
        let mixer = AudioMixer::new(2, SAMPLE_RATE);
        let proxy = mixer.proxy();
        let spec = AudioSpecDesired {
            freq: Some(SAMPLE_RATE as i32),
            channels: Some(2),
            // ~23 ms: low enough for responsive sound effects, big enough
            // that the mixer thread doesn't underrun while a frame runs.
            samples: Some(1024),
        };
        let device = sdl2_audio.open_playback(None, &spec, |_| MixerCallback { proxy })?;
        device.resume();
        Ok(Self { device, mixer })
    }
}

impl AudioBackend for SdlAudioBackend {
    impl_audio_mixer_backend!(mixer);

    fn play(&mut self) {
        self.device.resume();
    }

    fn pause(&mut self) {
        self.device.pause();
    }
}
