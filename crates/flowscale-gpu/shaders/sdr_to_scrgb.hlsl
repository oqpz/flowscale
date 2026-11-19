cbuffer ColorParams : register(b0)
{
    uint2 FrameSize;
    float SdrWhiteScale;
    float Padding;
};
Texture2D<float4> InputFrame : register(t0);
RWTexture2D<float4> OutputFrame : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (any(tid.xy >= FrameSize)) return;
    float4 encoded = InputFrame.Load(int3(tid.xy, 0));
    float3 decoded = float3(
        encoded.r <= 0.04045f ? encoded.r / 12.92f : pow(max((encoded.r + 0.055f) / 1.055f, 0.0f), 2.4f),
        encoded.g <= 0.04045f ? encoded.g / 12.92f : pow(max((encoded.g + 0.055f) / 1.055f, 0.0f), 2.4f),
        encoded.b <= 0.04045f ? encoded.b / 12.92f : pow(max((encoded.b + 0.055f) / 1.055f, 0.0f), 2.4f));
    OutputFrame[tid.xy] = float4(decoded * SdrWhiteScale, encoded.a);
}
