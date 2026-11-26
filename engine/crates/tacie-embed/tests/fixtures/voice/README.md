# Speaker embedding fixtures

Two speakers × two utterances from the Zeroth-Korean test set (CC BY 4.0,
https://huggingface.co/datasets/kresnik/zeroth_korean), the middle 3 seconds of each. Source utterance ids:
`a_1.wav` ← `104_003_0019`, `a_2.wav` ← `104_003_0193`, `b_1.wav` ← `105_003_0029`, `b_2.wav` ← `105_003_0478`.

`fbank.json` holds the first 20 frames from kaldi-native-fbank (before mean subtraction) and the frame count.
`embeddings.json` holds the normalized embeddings from sherpa-onnx `SpeakerEmbeddingExtractor` (ERes2Net base). The tests
compare the engine's own fbank and embeddings with these values.
