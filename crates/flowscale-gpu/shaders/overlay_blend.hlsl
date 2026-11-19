// Premultiplied alpha compute composite for cached FPS & cursor overlays (cs_5_0).
cbuffer OverlayBlendParams : register(b0)
{
    uint DstX;
    uint DstY;
    uint DstW;
    uint DstH;
    uint SrcW;
    uint SrcH;
    float SrcScaleX;
    float SrcScaleY;
    float SrcOffX;
    float SrcOffY;
    float HdrWhiteScale;
    uint IsHdr;
};

Texture2D<float4> OverlayTex : register(t0);
SamplerState LinearClamp : register(s0);
RWTexture2D<float4> BackBuffer : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= DstW || tid.y >= DstH)
        return;

    uint2 bbDim;
    BackBuffer.GetDimensions(bbDim.x, bbDim.y);
    uint2 dstCoord = uint2(DstX, DstY) + tid.xy;
    if (any(dstCoord >= bbDim))
        return;

    float2 srcSize = float2(SrcW, SrcH) * float2(SrcScaleX, SrcScaleY);
    if (srcSize.x <= 0.0f || srcSize.y <= 0.0f)
        return;

    float2 uv = (float2(tid.xy) + 0.5f - float2(SrcOffX, SrcOffY)) / srcSize;
    if (any(uv < 0.0f) || any(uv > 1.0f))
        return;

    float4 src = OverlayTex.SampleLevel(LinearClamp, uv, 0);
    if (src.a <= 0.0f)
        return;

    float3 srcRgb = src.rgb;
    if (IsHdr != 0u)
    {
        float3 straight = srcRgb / max(src.a, 1e-4f);
        srcRgb = float3(
            straight.r <= 0.04045f ? straight.r / 12.92f : pow(max((straight.r + 0.055f) / 1.055f, 0.0f), 2.4f),
            straight.g <= 0.04045f ? straight.g / 12.92f : pow(max((straight.g + 0.055f) / 1.055f, 0.0f), 2.4f),
            straight.b <= 0.04045f ? straight.b / 12.92f : pow(max((straight.b + 0.055f) / 1.055f, 0.0f), 2.4f)) * src.a;
    }

    float4 dst = BackBuffer[dstCoord];
    float invA = 1.0f - saturate(src.a);
    float3 outRgb = srcRgb * HdrWhiteScale + dst.rgb * invA;
    float outA = saturate(src.a + dst.a * invA);
    BackBuffer[dstCoord] = float4(outRgb, outA);
}
