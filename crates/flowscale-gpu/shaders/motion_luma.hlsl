cbuffer MotionParams : register(b0)
{
    uint2 FrameSize;
    uint2 FlowSize;
    int SearchRadius;
    uint BlockRadius;
    uint PassMode;
    uint CoarseStride;
};
Texture2D<float4> PrevFrame : register(t0);
Texture2D<float4> CurrFrame : register(t1);
SamplerState LinearClamp : register(s0);
RWTexture2D<float2> LumaPair : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (any(tid.xy >= FlowSize)) return;
    float2 uv = (float2(tid.xy) * 4.0f + 1.0f) / float2(FrameSize);
    float2 d = 1.0f / float2(FrameSize);
    const float3 weights = float3(0.2126f, 0.7152f, 0.0722f);
    float3 p = 0, c = 0;
    [unroll] for (int y = 0; y < 4; y += 2)
    {
        [unroll] for (int x = 0; x < 4; x += 2)
        {
            float2 at = uv + float2(x, y) * d;
            p += PrevFrame.SampleLevel(LinearClamp, at, 0).rgb;
            c += CurrFrame.SampleLevel(LinearClamp, at, 0).rgb;
        }
    }
    LumaPair[tid.xy] = float2(dot(p, weights), dot(c, weights)) * 0.25f;
}
